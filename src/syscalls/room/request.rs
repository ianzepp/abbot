//! Room:Request - Composite syscall: create → stream → run in single call
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall is a convenience wrapper that orchestrates the full room lifecycle
//! (create → stream → run) in a single call. It's designed for callers that want
//! to trigger an immediate conclave without managing the multi-step protocol.
//!
//! **Execution sequence:**
//! 1. Dispatch `room:create` with type=conclave, scope=main
//! 2. Wait for room_id in response
//! 3. Dispatch `room:stream` in background (for observability)
//! 4. Dispatch `room:run` with reason as context
//! 5. Wait for decision in response
//! 6. Cancel stream and return decision to caller
//!
//! **Use cases:**
//! - LLM tool calls: Agent decides it needs collective input on a decision
//! - CLI commands: User requests immediate reflection (e.g., "abbot reflect")
//! - API endpoints: External service triggers deliberation on specific event
//!
//! **Hardcoded defaults:**
//! - Room type: "conclave" (strategic planning session)
//! - Scope: "main" (primary workspace context)
//! - Wake mode: "normal" (full context loading)
//! - WHY hardcoded: room:request is convenience wrapper for common case. Callers
//!   needing custom room types/scopes should use explicit room:create + room:run.
//!
//! **Stream handling:**
//! - Stream is opened in background (detached, not awaited)
//! - WHY background: Stream is for observability (logging, debugging) not required
//!   for correctness. Room:run can proceed without consuming stream events.
//! - Stream is cancelled after room:run completes (prevents channel leak)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Simplicity over flexibility**: Hardcodes common defaults (conclave, main scope)
//!   to reduce API surface. Callers needing flexibility use explicit syscalls.
//! - **Synchronous behavior**: Blocks until room completes (unlike room:run which
//!   returns immediately after starting deliberation). Simplifies caller logic.
//! - **Fire-and-forget stream**: Opens stream for observability but doesn't consume
//!   events. Stream is background noise, not required for operation.
//!
//! CONCURRENCY
//! ===========
//! - Dispatches three syscalls sequentially (room:create, room:stream, room:run)
//! - Stream runs in background (doesn't block room:run)
//! - Multiple callers can invoke room:request concurrently (each gets separate room)
//!
//! SECURITY MODEL
//! ==============
//! - Inherits actor from calling context (uses ctx.actor for dispatched syscalls)
//! - WHY inherit: Ensures room execution has same permissions as caller
//! - Room:run uses system actor internally (bypasses per-call permission checks)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Hardcoded defaults vs configurable**: Room type, scope, wake_mode are
//!    hardcoded. This simplifies API but means callers needing custom configuration
//!    must use explicit room:create + room:run sequence.
//!
//! 2. **Synchronous vs asynchronous**: Blocks until room completes. This simplifies
//!    caller logic (single syscall returns decision) but means caller waits for
//!    full deliberation loop (potentially minutes for complex conclaves).
//!
//! 3. **Background stream vs no stream**: Opens stream in background for observability
//!    but doesn't consume events. This enables debugging without caller complexity
//!    but wastes resources if stream events are never observed.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::kernel::{Frame, FrameOp, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for room:request syscall.
///
/// WHY single field: Room type, scope, wake_mode are hardcoded (conclave, main,
/// normal). Only configurable parameter is reason (why room was requested).
#[derive(Deserialize)]
struct Args {
    /// Why the room was requested (e.g., "agent needs collective input on decision").
    ///
    /// WHY: Passed as context to room:run, visible in deliberation transcript and
    /// stream events. Enables debugging ("why did this conclave trigger?").
    reason: String,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Composite syscall that orchestrates full room lifecycle in single call.
///
/// WHY: Convenience wrapper for callers that want immediate conclave without
/// managing multi-step protocol (create → stream → run).
pub struct RoomRequest;

impl RoomRequest {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomRequest {
    fn name(&self) -> &'static str {
        "room:request"
    }

    /// Create, stream, and run a conclave room in single call.
    ///
    /// WHY this exists: Simplifies LLM tool calls and CLI commands that need
    /// immediate collective input. Single syscall reduces API surface and prevents
    /// caller errors (forgetting to open stream, wrong room:run arguments).
    ///
    /// ARGUMENTS:
    /// - `reason`: Why room was requested (visible in transcript and stream events)
    ///
    /// RETURNS:
    /// - `Frame::ok` with decision JSON from room:run (includes status, decision)
    /// - `E_INVALID_ARGS` if reason is empty
    /// - `E_INTERNAL` if kernel not initialized or room creation fails
    ///
    /// BEHAVIOR:
    /// - Dispatches room:create (type=conclave, scope=main)
    /// - Dispatches room:stream in background (for observability, not consumed)
    /// - Dispatches room:run (wake_mode=normal, context=reason)
    /// - Blocks until room:run completes
    /// - Cancels stream after room:run completes
    /// - Returns decision to caller
    ///
    /// USAGE:
    /// ```json
    /// {"reason": "agent needs collective input on whether to proceed with risky operation"}
    /// ```
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: Args = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid args: {e}")))?;

        if args.reason.trim().is_empty() {
            return Err(KernelError::invalid_args("reason is empty"));
        }

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let dispatcher = k.dispatcher().await;
        let actor = ctx
            .actor
            .as_deref()
            .unwrap_or("system")
            .to_string();

        // -------------------------------------------------------------------------
        // PHASE 1: CREATE ROOM
        // WHY: Allocates room_id needed for subsequent stream and run calls.
        // Hardcodes type=conclave, scope=main (common defaults).
        // -------------------------------------------------------------------------
        let create_req = Frame::req("room:create", json!({"type": "conclave", "scope": "main"}))
            .with_actor(actor.clone());
        let mut rx = dispatcher.dispatch(create_req, ctx.cwd.clone(), CancellationToken::new());

        let mut room_id = String::new();
        while let Some(frame) = rx.recv().await {
            if frame.op == FrameOp::Ok {
                if let Some(data) = &frame.data {
                    if let Some(id) = data.get("room_id").and_then(|v| v.as_str()) {
                        room_id = id.to_string();
                    }
                }
                break;
            }
            if matches!(frame.op, FrameOp::Error | FrameOp::Done) {
                break;
            }
        }

        if room_id.is_empty() {
            return Err(KernelError::internal("failed to create room"));
        }

        // -------------------------------------------------------------------------
        // PHASE 2: OPEN STREAM (background)
        // WHY: Enables observability (logging, debugging) without blocking room:run.
        // Stream events are not consumed by this syscall (fire-and-forget for
        // side-channel observation).
        // -------------------------------------------------------------------------
        let stream_cancel = CancellationToken::new();
        let _stream_rx = dispatcher.dispatch(
            Frame::req("room:stream", json!({"room_id": room_id})).with_actor(actor.clone()),
            ctx.cwd.clone(),
            stream_cancel.clone(),
        );

        // -------------------------------------------------------------------------
        // PHASE 3: RUN ROOM
        // WHY: Executes deliberation loop. Blocks until room completes (consensus,
        // timeout, or cancellation). Passes reason as context for deliberation.
        // -------------------------------------------------------------------------
        let run_req = Frame::req(
            "room:run",
            json!({"room_id": room_id, "wake_mode": "normal", "context": args.reason}),
        )
        .with_actor(actor);
        let mut run_rx = dispatcher.dispatch(run_req, ctx.cwd.clone(), CancellationToken::new());

        let mut result = json!({"requested": true, "reason": args.reason});
        while let Some(frame) = run_rx.recv().await {
            if frame.op == FrameOp::Ok {
                result = frame.data.unwrap_or(json!({"requested": true}));
                break;
            }
            if matches!(frame.op, FrameOp::Error | FrameOp::Done) {
                break;
            }
        }

        // WHY cancel stream: Room:run completed, no more events will be emitted.
        // Cancelling stream prevents channel leak (stream task terminates).
        stream_cancel.cancel();

        let _ = tx.send(Frame::ok(ctx.call_id, result)).await;
        Ok(())
    }
}
