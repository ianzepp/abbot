//! Chat:Done - Signal turn completion and close conversation stream
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall signals successful turn completion and closes the conversation stream for
//! a specific (room, reply_to) pair. It implements the **graceful termination path** for
//! agent turns: agent finishes responding → chat:done broadcasts completion → Sigcalls
//! stream closes → subscribers stop listening.
//!
//! **Critical design decisions:**
//! - Supports two completion reasons: "complete" (fully answered) and "awaiting_tools" (paused for tool execution)
//! - Emits both Frame::item (completion metadata) and Frame::done (stream termination signal)
//! - Closes Sigcalls stream to prevent resource leaks (subscribers detach)
//! - No logging (completion is ephemeral state, not chat history)
//!
//! **Integration points:**
//! - `Sigcalls` - Real-time broadcast of completion events and stream closure
//!
//! **Frame protocol:**
//! - Emits `Frame::item` (type: done, reason: complete/awaiting_tools) for completion metadata
//! - Emits `Frame::done` to signal stream termination
//! - Calls `Sigcalls.close()` to clean up subscriber resources
//! - Returns `Frame::ok` to caller with `{"closed": true}` acknowledgment
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Two-stage broadcast**: Frame::item (metadata) + Frame::done (termination)
//! - **Resource cleanup**: close() prevents orphaned subscribers consuming memory
//! - **No actor restrictions**: Any agent may signal completion (head, hand, room)
//! - **Reason validation**: Only "complete" and "awaiting_tools" are valid (prevents malformed signals)
//!
//! CONCURRENCY
//! ===========
//! - **Lane assignment**: Immediate lane (turn completion is user-facing event)
//! - **Sigcalls broadcast**: Async send (no blocking)
//! - **Stream closure**: Immediate (subscribers notified synchronously)
//!
//! COMPLETION REASONS
//! ==================
//! WHY two different completion reasons:
//!
//! 1. **"complete"**:
//!    - Agent has fully responded to user's request
//!    - No further interaction needed
//!    - UI can display "Turn complete" or hide loading indicator
//!
//! 2. **"awaiting_tools"**:
//!    - Agent has emitted tool calls (chat:tool) and is pausing for results
//!    - Turn is NOT complete, but agent has yielded control
//!    - UI can display "Waiting for tool results" or show tool execution progress
//!    - When tool results arrive (chat:tool_result), agent resumes and may emit new text/tools
//!
//! WHY separate reason vs. implicit completion:
//! - "awaiting_tools" allows UI to distinguish pause vs. completion
//! - Enables tool execution progress indicators (user sees agent is working)
//! - Future extension: additional reasons like "awaiting_user_input" or "rate_limited"
//!
//! FRAME SEQUENCE
//! ==============
//! WHY emit both Frame::item and Frame::done:
//!
//! 1. **Frame::item (type: done, reason: X)**:
//!    - Carries completion metadata (reason field)
//!    - Subscribers can display different UI based on reason
//!    - Follows same pattern as other Frame::item events (tool_call, text_delta)
//!
//! 2. **Frame::done (no data)**:
//!    - Universal stream termination signal (no metadata)
//!    - Subscribers know to stop listening (no more events)
//!    - Consistent with Rust stream conventions (None = end of stream)
//!
//! WHY both instead of just Frame::done:
//! - Frame::item provides reason (complete vs. awaiting_tools)
//! - Frame::done is universal termination (works for errors, cancellations too)
//! - Separation enables future metadata expansion (Frame::item) without changing termination protocol

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TurnKey};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_room};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for signaling turn completion and closing conversation stream.
///
/// WHY: Zero-sized struct (stateless). All stream management is in execute().
pub struct ChatDone;

#[async_trait]
impl Syscall for ChatDone {
    fn name(&self) -> &'static str {
        "chat:done"
    }

    /// Signal turn completion and close conversation stream.
    ///
    /// WHY: Enables agents to gracefully terminate conversation turns and clean
    /// up subscriber resources. Prevents orphaned stream listeners consuming memory.
    ///
    /// USE CASE:
    /// - Agent finishes responding to user request → chat:done(reason: "complete")
    /// - Agent emits tool calls and pauses → chat:done(reason: "awaiting_tools")
    /// - Subscribers receive Frame::item (metadata) + Frame::done (termination)
    /// - Sigcalls.close() detaches subscribers and frees resources
    ///
    /// SECURITY NOTE: No actor restrictions (any agent may complete turns).
    /// Reason must be "complete" or "awaiting_tools" (prevents malformed signals).
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"closed": true}` on successful stream closure
    /// - `E_INVALID_ARGS` if reason is not "complete" or "awaiting_tools"
    /// - `E_CANCELLED` if context is cancelled mid-execution
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Argument Validation
        // =====================================================================
        // WHY: Validate room, reply_to, and reason before stream closure.
        // Early cancellation check prevents wasted work on cancelled contexts.
        ctx.check_cancelled()?;

        let room = parse_room(&data)?;
        let reply_to = parse_reply_to(&data)?;

        // WHY: Reason must be one of two valid values: "complete" or "awaiting_tools".
        // Rejects other values to prevent malformed completion signals (e.g., "error"
        // should use chat:error instead, "cancelled" should use chat:cancel).
        let reason = data
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if reason != "complete" && reason != "awaiting_tools" {
            return Err(KernelError::invalid_args("invalid reason"));
        }

        // =====================================================================
        // PHASE 2: Completion Metadata Broadcast
        // =====================================================================
        // WHY: Emit Frame::item BEFORE Frame::done. Subscribers receive metadata
        // (completion reason) before stream termination. Enables UI to distinguish
        // "complete" (hide loading) vs. "awaiting_tools" (show tool progress).
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        k.sigcalls()
            .send(
                room,
                reply_to,
                Frame::item(ctx.call_id, json!({"type": "done", "reason": reason}))
                    .with_name("chat:done")
                    .with_actor(ctx.actor_str().to_string()),
            )
            .await;

        // =====================================================================
        // PHASE 3: Stream Termination Signal
        // =====================================================================
        // WHY: Emit Frame::done to signal stream termination. Universal signal
        // (no metadata) tells subscribers to stop listening. Consistent with
        // Rust stream conventions (None = end of stream).
        k.sigcalls()
            .send(
                room,
                reply_to,
                Frame::done(ctx.call_id)
                    .with_name("chat:done")
                    .with_actor(ctx.actor_str().to_string()),
            )
            .await;

        // =====================================================================
        // PHASE 4: Subscriber Resource Cleanup
        // =====================================================================
        // WHY: close() detaches all subscribers for (room, reply_to) and frees
        // associated resources (channels, buffers). Prevents memory leaks from
        // orphaned listeners waiting for events that will never arrive.
        //
        // IMPORTANT: close() MUST be called after Frame::done emission. Otherwise
        // subscribers may not receive termination signal (race condition).
        k.sigcalls().close(room, reply_to).await;

        // Only finish turn state when truly complete. "awaiting_tools" keeps
        // pending tool calls alive for result delivery.
        if reason == "complete" {
            k.turns().finish(&TurnKey::new(room, reply_to)).await;
        }

        // =====================================================================
        // PHASE 5: Acknowledgment
        // =====================================================================
        // WHY: Return Frame::ok to caller confirming stream was closed.
        // "closed" flag distinguishes from other syscall responses.
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"closed": true})))
            .await;
        Ok(())
    }
}
