//! Room:Run - Execute deliberation loop for a room
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall executes the multi-round deliberation loop for a room via `RoomRunner`.
//! It's the core operation that drives participant interaction, proposal tracking, voting,
//! and consensus detection. Room:run requires an open stream (via `room:stream`) to emit
//! progress events as deliberation unfolds.
//!
//! **Execution phases:**
//! 1. Validate room exists and stream is open
//! 2. Emit `room_start` event with context and wake_mode
//! 3. Construct `Room` struct matching room type (conclave/autonomy/work)
//! 4. Delegate to `RoomRunner::run()` for deliberation loop
//! 5. Emit `room_end` event with decision and transcript
//! 6. Close stream and return decision to caller
//!
//! **RoomRunner integration:**
//! - Runner handles: round iteration, participant sequencing, LLM queries, proposal
//!   tracking, consensus detection, worktree provisioning (for work rooms)
//! - Runner returns: `Option<RoomDecision>` (None if no consensus reached)
//! - Runner emits: participant_turn, proposal_made, vote_cast events to stream
//!
//! **Wake modes:**
//! - `normal`: Standard deliberation (load recent history, current state)
//! - `init`: Reboot mode (triggered after control:reboot_collective decision, loads
//!   minimal state to avoid circular dependency on previous collective state)
//!
//! **Decision execution:**
//! - Approved proposals are converted to needs and dispatched via `need:enqueue`
//! - Fire-and-forget semantics: room doesn't wait for need completion
//! - WHY fire-and-forget: Prevents rooms from blocking indefinitely on long-running
//!   operations (git clones, LLM queries, file processing)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Stream-first execution**: Requires stream to be open before running. This ensures
//!   no events are lost (stream is ready to receive before runner emits).
//! - **Delegated deliberation**: Syscall is a thin wrapper around RoomRunner. All
//!   deliberation logic lives in runner (enables testing without kernel).
//! - **Automatic cleanup**: Stream is closed automatically after deliberation completes
//!   (no manual cleanup required by caller).
//! - **Persistent transcripts**: Deliberation transcripts are persisted to `conclaves`
//!   table for historical analysis (survives stream closure).
//!
//! CONCURRENCY
//! ===========
//! - Room:run executes on Room lane (dedicated concurrency lane for room operations)
//! - Multiple rooms can run concurrently (bounded by Room lane concurrency limit)
//! - RoomRunner spawns detached tasks for LLM queries (doesn't block lane)
//! - Stream events are sent fire-and-forget (won't block if channel full)
//!
//! SECURITY MODEL
//! ==============
//! - No permission checks (any actor can run rooms)
//! - WHY permissive: Rooms are internal coordination mechanisms. RoomCoordinator
//!   controls when rooms execute based on idle detection policy.
//! - Room execution uses system actor for need dispatch (bypasses per-call checks)
//! - Worktree isolation prevents work rooms from modifying main workspace
//!
//! TRADE-OFFS
//! ==========
//! 1. **Stream-first vs stream-optional**: Requiring stream before run ensures no
//!    events are lost but adds coordination overhead. Chosen because observability
//!    is critical for debugging deliberation failures.
//!
//! 2. **Fire-and-forget decisions vs synchronous execution**: Approved proposals are
//!    dispatched as needs without waiting for completion. This prevents rooms from
//!    blocking indefinitely but means rooms can't react to decision outcomes.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, RoomKind, Syscall, SyscallContext};
use crate::runtime::{Kernel, Room, RoomConfig, RoomRunner, WakeMode};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for executing room deliberation loops via RoomRunner.
///
/// WHY: Thin wrapper around RoomRunner that integrates with kernel services (store,
/// coordinator, dispatcher) and emits stream events for observability.
pub struct RoomRun;

impl RoomRun {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomRun {
    fn name(&self) -> &'static str {
        "room:run"
    }

    /// Execute deliberation loop for a room.
    ///
    /// WHY this exists: Drives multi-round participant interaction to reach consensus
    /// on proposals (control changes, self-model updates, needs). Delegates to
    /// RoomRunner for deliberation logic while handling kernel integration (stream
    /// events, transcript persistence, decision dispatch).
    ///
    /// ARGUMENTS:
    /// - `room_id`: UUID of room to run (must exist via room:create first)
    /// - `wake_mode`: "normal" or "init" (init skips loading previous collective state)
    /// - `context`: Optional string describing why room was triggered
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{room_id, status, decision}` on completion
    /// - `E_NOT_FOUND` if room_id doesn't exist
    /// - `E_INVALID_ARGS` if stream not opened (must call room:stream first)
    /// - `E_INTERNAL` if kernel store not attached
    ///
    /// BEHAVIOR:
    /// - Emits room_start event when deliberation begins
    /// - Delegates to RoomRunner::run() for deliberation loop
    /// - Emits room_end event with decision and transcript
    /// - Closes stream automatically on completion
    /// - Returns decision (or None if no consensus reached)
    ///
    /// USAGE:
    /// ```json
    /// {"room_id": "550e8400-e29b-41d4-a716-446655440000", "wake_mode": "normal", "context": "slow idle detected"}
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

        let rec = k
            .rooms()
            .get(room_id)
            .await
            .ok_or_else(|| KernelError::not_found("room not found"))?;

        // WHY require stream first: Ensures no events are lost. If stream isn't open,
        // room_start and subsequent events would be dropped. Requiring stream-first
        // guarantees observability from the start of deliberation.
        if !k.rooms().has_stream(room_id).await {
            return Err(KernelError::invalid_args(
                "room stream not opened (call room:stream before room:run)",
            ));
        }

        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        // WHY wake modes: "init" skips loading previous collective state to avoid
        // circular dependency after control:reboot_collective decision. "normal" loads
        // full context (recent history, current state, memory files).
        let wake_mode = data
            .get("wake_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("normal")
            .trim();
        let wake_mode = match wake_mode {
            "init" => WakeMode::Init,
            _ => WakeMode::Normal,
        };

        let context = data.get("context").and_then(|v| v.as_str()).unwrap_or("");

        // -------------------------------------------------------------------------
        // PHASE 1: EMIT START EVENT
        // WHY emit before running: Enables stream observers to see deliberation
        // context (type, scope, wake_mode) before first participant speaks.
        // -------------------------------------------------------------------------
        let _ = k
            .rooms()
            .send(
                room_id,
                Frame::event(
                    room_id,
                    json!({"kind": "room_start", "room_id": room_id.to_string(), "type": rec.kind.as_str(), "context": context, "wake_mode": format!("{:?}", wake_mode)}),
                ),
            )
            .await;

        // -------------------------------------------------------------------------
        // PHASE 2: CONSTRUCT ROOM AND RUNNER
        // WHY separate construction: Room struct defines deliberation parameters
        // (participants, round limits) while RoomRunner provides execution context
        // (store, workspace, config). Separating them enables testing runner with
        // different room configurations.
        // -------------------------------------------------------------------------
        let scopes = vec![crate::Scope::from(rec.scope.as_str())];
        let room_id_str = room_id.to_string();
        let room_cfg = RoomConfig::from_config();
        let runner = RoomRunner::new(store.clone(), scopes, k.workspace().to_path_buf(), room_cfg);

        // WHY match on room kind: Each room type has different participant sets and
        // round limits (conclave=5, autonomy=3, work=10). Room::conclave/autonomy/work
        // constructors encode these defaults.
        let mut room = match rec.kind {
            RoomKind::Conclave => Room::conclave(&room_id_str),
            RoomKind::Autonomy => Room::autonomy(&room_id_str),
            RoomKind::Work => Room::work(&room_id_str),
        };

        // -------------------------------------------------------------------------
        // PHASE 3: EXECUTE DELIBERATION LOOP
        // WHY runner.run: Delegates all deliberation logic (round iteration,
        // participant sequencing, LLM queries, consensus detection) to RoomRunner.
        // Runner emits events to stream as deliberation progresses.
        // -------------------------------------------------------------------------
        let decision = runner.run(&mut room, None).await;

        // -------------------------------------------------------------------------
        // PHASE 4: EMIT END EVENT WITH TRANSCRIPT
        // WHY load from store: Transcript is persisted during deliberation.
        // Loading from store ensures we return the canonical persisted version
        // (not an in-memory copy that might differ).
        // -------------------------------------------------------------------------
        let record = store.get_conclave(&room_id_str).ok().flatten();
        let _ = k
            .rooms()
            .send(
                room_id,
                Frame::event(
                    room_id,
                    json!({
                        "kind": "room_end",
                        "room_id": room_id_str,
                        "status": record.as_ref().map(|r| r.status.clone()).unwrap_or_else(|| if decision.is_some() {"done".to_string()} else {"no_decision".to_string()}),
                        "decision": decision,
                        "transcript": record.as_ref().and_then(|r| serde_json::from_str::<serde_json::Value>(&r.transcript).ok()),
                    }),
                ),
            )
            .await;

        // -------------------------------------------------------------------------
        // PHASE 5: CLOSE STREAM
        // WHY close automatically: Stream lifecycle is tied to room execution.
        // Closing here ensures stream consumers receive terminal event and don't
        // wait indefinitely for more events.
        // -------------------------------------------------------------------------
        let _ = k
            .rooms()
            .send(room_id, Frame::ok(room_id, json!({"status": "closed"})))
            .await;
        k.rooms().close_stream(room_id).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"room_id": room_id.to_string(), "status": "done", "decision": decision}),
            ))
            .await;
        Ok(())
    }
}
