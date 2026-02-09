//! Chat:Cancel - Cancel in-flight agent turn via TurnTracker
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall cancels an in-flight agent turn, triggering graceful shutdown of associated
//! tasks (LLM requests, tool executions, etc.). It implements the **user-initiated cancellation
//! path**: user clicks "Stop" button → chat:cancel triggers TurnTracker cancellation → tasks
//! observe cancellation token and abort → resources are cleaned up.
//!
//! **Critical design decisions:**
//! - Does NOT emit Frame::error or Frame::done (cancellation is internal state change)
//! - Does NOT close Sigcalls stream (turn may still emit final events during shutdown)
//! - Cancellation is cooperative (tasks must check cancellation token, not forced kill)
//! - Reason is informational only (logged for debugging, not enforced)
//!
//! **Integration points:**
//! - `TurnTracker.cancel()` - Marks turn as cancelled and notifies waiting tasks
//!
//! **Frame protocol:**
//! - Returns `Frame::ok` to caller with `{"cancelled": true}` acknowledgment
//! - Does NOT emit Frame::error or Frame::done via Sigcalls (internal state change only)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Graceful cancellation**: Tasks observe cancellation token and shut down cleanly
//! - **No forced termination**: Cancellation is cooperative (prevents resource leaks)
//! - **No stream closure**: Turn may emit final events (e.g., "Cancelled by user")
//! - **Reason tracking**: Optional reason field for debugging (default: "client_disconnect")
//!
//! CONCURRENCY
//! ===========
//! - **Lane assignment**: Immediate lane (cancellation is user-facing operation)
//! - **TurnTracker access**: Thread-safe (Arc<Mutex<...>>), cancellation is atomic
//! - **Cancellation propagation**: Async (tasks observe token at next check_cancelled() call)
//!
//! CANCELLATION VS. ERROR
//! ======================
//! WHY separate chat:cancel from chat:error:
//!
//! **chat:cancel** (user-initiated abort):
//! - User explicitly requests turn cancellation (e.g., clicks "Stop" button)
//! - Triggers TurnTracker.cancel() → tasks abort gracefully
//! - Does NOT emit Frame::error (cancellation is expected, not failure)
//! - Turn may still emit final events (e.g., "Cancelled by user" message)
//! - Stream is NOT closed (agent may send final cleanup messages)
//!
//! **chat:error** (agent-detected failure):
//! - Agent encounters unrecoverable error (timeout, API failure, etc.)
//! - Emits Frame::error with error details → stream closes immediately
//! - Tasks may still be running (cancellation happens as side effect)
//!
//! WHY no Frame::error for cancellation:
//! - Cancellation is user-initiated (not a failure condition)
//! - UI should show "Cancelled by user" (not "Error: Turn cancelled")
//! - Agent may send custom cancellation message via chat:message
//!
//! COOPERATIVE CANCELLATION
//! ========================
//! WHY cancellation is cooperative (not forced):
//!
//! **COOPERATIVE** (current design):
//! - Tasks check ctx.check_cancelled() periodically
//! - On cancellation, tasks return E_CANCELLED error
//! - Resources are cleaned up properly (files closed, locks released, etc.)
//!
//! **FORCED** (not implemented):
//! - Cancellation immediately kills task thread
//! - Resources may leak (files left open, temp files not deleted)
//! - In-flight HTTP requests may not be aborted
//!
//! WHY cooperative is safer:
//! - Prevents resource leaks (tasks clean up properly)
//! - Enables graceful shutdown (LLM requests can be aborted cleanly)
//! - Consistent with Rust async cancellation model (drop Future = cancel)
//!
//! CANCELLATION REASON
//! ===================
//! WHY track cancellation reason:
//!
//! **Common reasons**:
//! - "client_disconnect" (default) - User closed browser tab or disconnected
//! - "user_request" - User explicitly clicked "Stop" button
//! - "timeout" - Turn exceeded maximum duration
//! - "new_turn" - User started new turn (implicitly cancels previous)
//!
//! WHY reason is informational only:
//! - TurnTracker.cancel() does not enforce different behavior based on reason
//! - Reason is logged for debugging (why was turn cancelled?)
//! - Future extension: metrics tracking (cancellation rate by reason)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TurnKey};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_room};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for cancelling in-flight agent turns via TurnTracker.
///
/// WHY: Zero-sized struct (stateless). All cancellation logic is in execute().
pub struct ChatCancel;

#[async_trait]
impl Syscall for ChatCancel {
    fn name(&self) -> &'static str {
        "chat:cancel"
    }

    /// Cancel an in-flight agent turn.
    ///
    /// WHY: Enables users to abort long-running or misbehaving agent turns.
    /// Triggers graceful shutdown via TurnTracker cancellation token.
    ///
    /// USE CASE:
    /// - User clicks "Stop" button in UI → chat:cancel(reason: "user_request")
    /// - User closes browser tab → chat:cancel(reason: "client_disconnect")
    /// - User starts new turn → chat:cancel(reason: "new_turn")
    /// - TurnTracker marks turn as cancelled → tasks observe token and abort
    ///
    /// SECURITY NOTE: No actor restrictions (any actor may cancel turns).
    /// Cancellation is cooperative (tasks must check cancellation token).
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"cancelled": true}` on successful cancellation
    /// - Does NOT emit Frame::error or Frame::done (cancellation is internal state)
    /// - Does NOT close Sigcalls stream (turn may emit final events)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Argument Validation
        // =====================================================================
        // WHY: Validate scope and reply_to before TurnTracker interaction.
        // Early cancellation check prevents wasted work (though cancelling a
        // cancellation is rare in practice).
        ctx.check_cancelled()?;

        let room = parse_room(&data)?;
        let reply_to = parse_reply_to(&data)?;

        // WHY: Cancellation reason is optional (defaults to "client_disconnect").
        // Informational only - does not affect cancellation behavior, only logging.
        let reason = data
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("client_disconnect")
            .trim();

        // =====================================================================
        // PHASE 2: TurnTracker Cancellation
        // =====================================================================
        // WHY: TurnTracker.cancel() marks turn as cancelled and notifies waiting
        // tasks via cancellation token. Tasks observe token at next check_cancelled()
        // call and abort gracefully.
        //
        // CONCURRENCY: Cancellation is thread-safe (Arc<Mutex<...>>). Multiple
        // concurrent cancel() calls are idempotent (turn is only cancelled once).
        //
        // IMPORTANT: cancel() does NOT emit Frame::error or Frame::done. Agent
        // may still send final messages (e.g., "Cancelled by user") before turn
        // completes. Stream closure is agent's responsibility (chat:done or chat:error).
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let key = TurnKey::new(room, reply_to);
        k.turns().cancel(&key, reason).await;

        // =====================================================================
        // PHASE 3: Acknowledgment
        // =====================================================================
        // WHY: Return Frame::ok to caller confirming cancellation was triggered.
        // "cancelled" flag distinguishes from other syscall responses.
        //
        // NOTE: Cancellation is asynchronous - tasks may still be running when
        // this syscall returns. Caller should not assume immediate termination.
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"cancelled": true})))
            .await;
        Ok(())
    }
}
