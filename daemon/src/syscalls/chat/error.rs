//! Chat:Error - Signal turn error and close conversation stream
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall signals turn failure and closes the conversation stream for a specific
//! (scope, reply_to) pair. It implements the **error termination path** for agent turns:
//! agent encounters unrecoverable error → chat:error broadcasts error details → Sigcalls
//! stream closes → subscribers display error to user.
//!
//! **Critical design decisions:**
//! - Requires both error code (e.g., "E_TIMEOUT", "E_INTERNAL") and human-readable message
//! - Emits Frame::error (not Frame::item) to distinguish from normal completion
//! - Closes Sigcalls stream to prevent resource leaks (same as chat:done)
//! - No logging (errors are propagated to subscribers, not persisted to chat history)
//!
//! **Integration points:**
//! - `Sigcalls` - Real-time broadcast of error details and stream closure
//!
//! **Frame protocol:**
//! - Emits `Frame::error` with `{code, message}` for error details
//! - Calls `Sigcalls.close()` to clean up subscriber resources
//! - Returns `Frame::ok` to caller with `{"closed": true}` acknowledgment
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Structured errors**: code + message (enables programmatic error handling)
//! - **Resource cleanup**: close() prevents orphaned subscribers consuming memory
//! - **No actor restrictions**: Any agent may signal errors (head, hand, room)
//! - **Immediate termination**: No Frame::done (Frame::error implies termination)
//!
//! CONCURRENCY
//! ===========
//! - **Lane assignment**: Immediate lane (errors are user-facing events)
//! - **Sigcalls broadcast**: Async send (no blocking)
//! - **Stream closure**: Immediate (subscribers notified synchronously)
//!
//! ERROR VS. DONE
//! ==============
//! WHY separate chat:error from chat:done:
//!
//! **chat:done** (graceful completion):
//! - Agent successfully completed request (reason: "complete")
//! - OR agent is pausing for tool execution (reason: "awaiting_tools")
//! - Emits Frame::item + Frame::done (two-stage broadcast)
//! - UI displays success indicator or tool progress
//!
//! **chat:error** (failure termination):
//! - Agent encountered unrecoverable error (timeout, internal failure, etc.)
//! - Emits Frame::error only (no Frame::done, error implies termination)
//! - UI displays error message to user
//!
//! WHY Frame::error instead of Frame::item:
//! - Frame::error is semantically different (failure vs. success)
//! - Subscribers can handle errors differently (display red banner vs. green checkmark)
//! - Consistent with kernel error propagation (Frame::error used throughout syscalls)
//!
//! ERROR CODE CONVENTIONS
//! ======================
//! WHY require both code and message:
//!
//! **code** (machine-readable):
//! - Enables programmatic error handling (e.g., retry on "E_TIMEOUT", abort on "E_INTERNAL")
//! - Consistent with KernelError code field (e.g., "E_TIMEOUT", "E_CANCELLED")
//! - Allows UI to show localized error messages based on code
//!
//! **message** (human-readable):
//! - Provides context for debugging (e.g., "Request timed out after 30s")
//! - Displayed to user as fallback if code is unknown
//! - Includes specifics (file paths, command names, etc.)
//!
//! EXAMPLE ERRORS:
//! - `{"code": "E_TIMEOUT", "message": "Agent response timed out after 30s"}`
//! - `{"code": "E_INTERNAL", "message": "LLM API returned invalid JSON"}`
//! - `{"code": "E_RATE_LIMIT", "message": "OpenAI rate limit exceeded, retry in 60s"}`

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_scope};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for signaling turn error and closing conversation stream.
///
/// WHY: Zero-sized struct (stateless). All error handling is in execute().
pub struct ChatError;

#[async_trait]
impl Syscall for ChatError {
    fn name(&self) -> &'static str {
        "chat:error"
    }

    /// Signal turn error and close conversation stream.
    ///
    /// WHY: Enables agents to report unrecoverable errors and gracefully terminate
    /// conversation turns. Subscribers receive error details and know to stop listening.
    ///
    /// USE CASE:
    /// - Agent request times out → chat:error(code: "E_TIMEOUT", message: "...")
    /// - LLM API returns invalid response → chat:error(code: "E_INTERNAL", message: "...")
    /// - Rate limit exceeded → chat:error(code: "E_RATE_LIMIT", message: "...")
    /// - Subscribers receive Frame::error and display error message to user
    /// - Sigcalls.close() detaches subscribers and frees resources
    ///
    /// SECURITY NOTE: No actor restrictions (any agent may signal errors).
    /// Error code and message must be non-empty (prevents malformed error signals).
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"closed": true}` on successful stream closure
    /// - `E_INVALID_ARGS` if code or message is empty
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
        // WHY: Validate scope, reply_to, code, and message before stream closure.
        // Early cancellation check prevents wasted work on cancelled contexts.
        ctx.check_cancelled()?;

        let scope = parse_scope(&data)?;
        let reply_to = parse_reply_to(&data)?;

        // WHY: Error code is required for programmatic error handling.
        // Convention: uppercase with underscore (e.g., "E_TIMEOUT", "E_INTERNAL").
        // Consistent with KernelError code field.
        let code = data
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if code.is_empty() {
            return Err(KernelError::invalid_args("code is required"));
        }

        // WHY: Human-readable message is required for user-facing error display.
        // Should include context (e.g., timeout duration, API endpoint, file path).
        let message = data
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if message.is_empty() {
            return Err(KernelError::invalid_args("message is required"));
        }

        // =====================================================================
        // PHASE 2: Error Broadcast
        // =====================================================================
        // WHY: Emit Frame::error (not Frame::item) to signal failure. Subscribers
        // can distinguish errors from normal events and display error UI accordingly.
        //
        // IMPORTANT: Frame::error implies termination (no Frame::done needed).
        // Subscribers know to stop listening after receiving Frame::error.
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::error(ctx.call_id, json!({"code": code, "message": message}))
                    .with_name("chat:error")
                    .with_actor(ctx.actor_str().to_string()),
            )
            .await;

        // =====================================================================
        // PHASE 3: Subscriber Resource Cleanup
        // =====================================================================
        // WHY: close() detaches all subscribers for (scope, reply_to) and frees
        // associated resources. Prevents memory leaks from orphaned listeners.
        //
        // IMPORTANT: close() MUST be called after Frame::error emission. Otherwise
        // subscribers may not receive error signal (race condition).
        k.sigcalls().close(scope, reply_to).await;

        // =====================================================================
        // PHASE 4: Acknowledgment
        // =====================================================================
        // WHY: Return Frame::ok to caller confirming stream was closed.
        // "closed" flag is consistent with chat:done response.
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"closed": true})))
            .await;
        Ok(())
    }
}
