//! Room:Stream - Open bidirectional event channel for room observation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall opens a persistent event stream for observing room deliberation in
//! real-time. The stream remains open until the room completes (consensus reached,
//! timeout, or cancellation), forwarding all events emitted by `room:run` to the caller.
//!
//! **Stream lifecycle:**
//! 1. Caller invokes `room:stream` with room_id (must exist via `room:create` first)
//! 2. RoomCoordinator allocates in-memory channel (mpsc) for this room
//! 3. Stream syscall blocks, forwarding events as they arrive
//! 4. Room:run emits events (room_start, participant_turn, proposal_made, vote_cast, etc.)
//! 5. Stream terminates when room:run sends terminal frame (Frame::ok/error/done)
//! 6. Stream sends Frame::done to caller and closes channel
//!
//! **Event types emitted:**
//! - `room_start`: Room execution begins (includes type, scope, wake_mode)
//! - `participant_turn`: Participant begins deliberation turn
//! - `proposal_made`: Participant submits proposal (control, self, ltm, need)
//! - `vote_cast`: Participant votes on existing proposal
//! - `consensus_reached`: All participants signal "done" (deliberation ends early)
//! - `room_end`: Room execution completes (includes status, decision, transcript)
//!
//! **Integration points:**
//! - `RoomCoordinator::open_stream()` - Allocates channel for room_id
//! - `RoomCoordinator::send()` - Emits events to all open streams for room_id
//! - `RoomCoordinator::close_stream()` - Closes channel after room completion
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Non-blocking observation**: Stream syscall doesn't interfere with room execution.
//!   Room:run proceeds independently, emitting events as deliberation progresses.
//! - **Automatic cleanup**: Stream closes automatically when room completes (no manual
//!   cleanup required). RoomCoordinator guarantees channels are closed after room:run.
//! - **Parent frame chaining**: All forwarded events have parent_id set to stream call_id,
//!   enabling frame tracing through nested syscalls.
//! - **Cancellation propagation**: If stream caller cancels, stream closes but room
//!   continues running (observation is optional, not required for room execution).
//!
//! CONCURRENCY
//! ===========
//! - Stream syscall blocks until room completion (synchronous forwarding)
//! - Multiple callers can open streams for the same room_id (broadcast semantics)
//! - WHY broadcast: Enables multiple observers (e.g., CLI UI + logging service)
//! - Channel is mpsc (multi-producer, single-consumer per stream)
//!
//! SECURITY MODEL
//! ==============
//! - No permission checks (any actor can observe rooms)
//! - WHY permissive: Rooms are internal coordination mechanisms. Observability is
//!   critical for debugging and monitoring but doesn't grant control over execution.
//! - Stream events contain full deliberation transcript (proposals, votes, decisions)
//! - TRADE-OFF: No per-room access control. All actors see all room events.
//!
//! TRADE-OFFS
//! ==========
//! 1. **Synchronous forwarding vs buffering**: Stream forwards events immediately
//!    without buffering. This ensures low-latency observation but means slow consumers
//!    (network clients) can block event emission. Acceptable because room:run uses
//!    fire-and-forget sends (won't block on full channel).
//!
//! 2. **Ephemeral streams vs persistent logs**: Streams are in-memory only and close
//!    on room completion. Callers must consume events in real-time or lose them.
//!    Deliberation transcripts ARE persisted to SQLite for historical analysis.

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::kernel::{Frame, FrameOp, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for opening event streams to observe room deliberation.
///
/// WHY: Enables real-time observation of deliberation progress without blocking
/// room execution. Critical for building UIs that show participant turns, proposals,
/// and votes as they happen.
pub struct RoomStream;

impl RoomStream {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomStream {
    fn name(&self) -> &'static str {
        "room:stream"
    }

    /// Open event stream for a room and forward events until completion.
    ///
    /// WHY this exists: Separates room observation from execution. Callers can
    /// monitor deliberation progress without blocking room:run or requiring room:run
    /// to know about all observers (broadcast semantics).
    ///
    /// ARGUMENTS:
    /// - `room_id`: UUID of room to observe (must exist via room:create first)
    ///
    /// RETURNS:
    /// - Emits `Frame::event` for each room event (room_start, participant_turn, etc.)
    /// - Emits `Frame::done` when room completes (terminal event received)
    /// - `E_NOT_FOUND` if room_id doesn't exist
    /// - `E_INTERNAL` if kernel not initialized
    ///
    /// BEHAVIOR:
    /// - Blocks until room completes or caller cancels
    /// - Sets parent_id on all forwarded frames for tracing
    /// - Terminates on first terminal frame (Ok/Error/Done)
    /// - Closes channel automatically on exit
    ///
    /// USAGE:
    /// ```json
    /// {"room_id": "550e8400-e29b-41d4-a716-446655440000"}
    /// ```
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let room_id = data
            .get("room_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| KernelError::invalid_args("room_id is required"))?;

        // WHY check existence first: Fail fast if room_id doesn't exist rather than
        // opening stream and waiting indefinitely. Provides clear error message.
        if k.rooms().get(room_id).await.is_none() {
            return Err(KernelError::not_found("room not found"));
        }

        // WHY RoomCoordinator::open_stream: Allocates dedicated mpsc channel for this
        // stream. Multiple streams can be opened for the same room (broadcast semantics).
        let rx = k.rooms().open_stream(room_id).await;
        let mut stream = ReceiverStream::new(rx);

        // -------------------------------------------------------------------------
        // FORWARDING LOOP
        // WHY synchronous forwarding: Ensures low-latency event delivery. Stream
        // doesn't buffer events (caller sees them immediately as room emits).
        // -------------------------------------------------------------------------
        while let Some(mut frame) = stream.next().await {
            // WHY set parent_id: Enables frame tracing through nested syscalls. All
            // events forwarded by this stream are logically children of the stream call.
            frame.parent_id = Some(ctx.call_id);

            let is_terminal = matches!(frame.op, FrameOp::Ok | FrameOp::Error | FrameOp::Done);
            let _ = tx.send(frame).await;

            // WHY break on terminal: Room:run sends terminal frame when deliberation
            // completes. No more events will arrive, so close stream immediately.
            if is_terminal {
                break;
            }

            // WHY check cancellation: If caller cancels stream, close immediately
            // rather than continuing to forward events. Room continues running
            // (observation is optional, not required for execution).
            if ctx.is_cancelled() {
                break;
            }
        }

        let _ = tx.send(Frame::done(ctx.call_id)).await;
        Ok(())
    }
}
