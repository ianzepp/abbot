//! Chat:ToolResult - Deliver external tool execution results to agents
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall delivers tool execution results from external executors back to agents.
//! It implements the **response side** of the tool coordination protocol: external executor
//! runs tool → chat:tool_result delivers result → agent receives result and continues processing.
//!
//! **Critical design decisions:**
//! - Tool results are delivered via TurnTracker (matches tool_call_id from chat:tool)
//! - Cancellation check AFTER delivery (prevents race condition: result delivered but ignored)
//! - Results are logged to history (enables tool call replay and debugging)
//! - No Sigcalls broadcast (result is delivered directly to agent, not UI subscribers)
//!
//! **Integration points:**
//! - `TurnTracker.deliver_external_tool_result()` - Delivers result to waiting agent
//! - `TurnTracker.is_cancelled()` - Checks if turn was cancelled during tool execution
//! - `FrameStore` - Centralized persistence of all frames (automatic via dispatcher)
//!
//! **Frame protocol:**
//! - Returns `Frame::ok` to caller with `{"delivered": true}` acknowledgment
//! - Does NOT emit Frame::item via Sigcalls (result is internal to agent, not UI-visible)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Deliver-then-check-cancellation**: Result delivery is guaranteed even if turn is cancelled
//! - **No actor restrictions**: Any actor may deliver tool results (external executors vary)
//! - **Logging after delivery**: Ensures result is delivered to agent before async logging
//! - **Error flag**: is_error distinguishes tool failure from success (agent handles differently)
//!
//! CONCURRENCY
//! ===========
//! - **Lane assignment**: Immediate lane (tool results unblock waiting agents)
//! - **TurnTracker access**: Thread-safe (Arc<Mutex<...>>), delivery is atomic
//! - **Cancellation race**: Check cancelled AFTER delivery (prevents lost results)
//!
//! CANCELLATION SEMANTICS
//! ======================
//! WHY check cancellation AFTER delivery:
//!
//! **RACE CONDITION SCENARIO**:
//! 1. Agent calls chat:tool → external executor starts running tool
//! 2. User cancels turn (chat:cancel) → TurnTracker marks turn as cancelled
//! 3. External executor finishes tool execution → chat:tool_result delivers result
//!
//! **WHY DELIVER-THEN-CHECK**:
//! - If check-before-deliver: result is lost, executor wasted work
//! - If deliver-then-check: agent receives result, can decide whether to use it
//! - Cancellation is a suggestion (graceful stop), not a hard abort
//!
//! **TRADE-OFF**: Agent may receive result for cancelled turn, but:
//! - Agent is responsible for checking cancellation state
//! - Result is logged to history (debugging, replay)
//! - Prevents executor work from being silently discarded

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TurnKey};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_room};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for delivering external tool execution results to agents.
///
/// WHY: Zero-sized struct (stateless). All delivery logic is in execute().
pub struct ChatToolResult;

#[async_trait]
impl Syscall for ChatToolResult {
    fn name(&self) -> &'static str {
        "chat:tool_result"
    }

    /// Deliver tool execution result from external executor to agent.
    ///
    /// WHY: Completes the tool coordination lifecycle started by chat:tool.
    /// External executors run tools outside agent context and deliver results
    /// via this syscall, which unblocks the waiting agent.
    ///
    /// USE CASE:
    /// - Agent calls chat:tool for "read_file" → external executor receives notification
    /// - Executor runs read_file tool and captures result
    /// - Executor calls chat:tool_result with content (file contents or error)
    /// - Agent receives result via TurnTracker and continues processing
    ///
    /// SECURITY NOTE: No actor restrictions (executors vary). Tool execution
    /// security is enforced by executor (e.g., VFS path validation for read_file).
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"delivered": true}` on successful result delivery
    /// - `E_INVALID_ARGS` if tool_call_id/name/content missing or mismatched
    /// - `E_CANCELLED` if turn was cancelled (checked AFTER delivery)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Argument Validation
        // =====================================================================
        // WHY: Validate all required fields before TurnTracker interaction.
        // Early failure prevents invalid results from being delivered.
        ctx.check_cancelled()?;

        let room = parse_room(&data)?;
        let reply_to = parse_reply_to(&data)?;

        // WHY: tool_call_id must match the ID from chat:tool registration.
        // TurnTracker uses this to route result to correct waiting agent.
        let tool_call_id = data
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if tool_call_id.is_empty() {
            return Err(KernelError::invalid_args("tool_call_id is required"));
        }

        // WHY: Tool name must match registration (validation check in TurnTracker).
        // Prevents result mismatch (e.g., read_file result delivered to write_file call).
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            return Err(KernelError::invalid_args("name is required"));
        }

        // WHY: Content is the tool execution result (string). May be success
        // output, error message, or structured JSON (as string).
        let content = data
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::invalid_args("content is required"))?
            .to_string();

        // WHY: is_error flag distinguishes tool failure from success. Agent
        // handles errors differently (e.g., retry, report to user).
        let is_error = data
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // =====================================================================
        // PHASE 2: Cancellation State Capture
        // =====================================================================
        // WHY: Capture cancellation state BEFORE delivery (but check AFTER).
        // This prevents race condition where turn is cancelled mid-delivery.
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let key = TurnKey::new(room, reply_to);
        let cancelled = k.turns().is_cancelled(&key).await;

        // =====================================================================
        // PHASE 3: TurnTracker Result Delivery
        // =====================================================================
        // WHY: Deliver result to TurnTracker BEFORE checking cancellation.
        // Ensures external executor's work is not silently discarded if turn
        // was cancelled during tool execution.
        //
        // SECURITY: TurnTracker.deliver_external_tool_result() validates:
        // - tool_call_id was registered via chat:tool
        // - name matches registered tool name
        // Returns error if validation fails (unregistered tool_call_id, name mismatch).
        k.turns()
            .deliver_external_tool_result(&key, tool_call_id, name, content.clone(), is_error)
            .await
            .map_err(KernelError::invalid_args)?;

        // =====================================================================
        // PHASE 4: Cancellation Check
        // =====================================================================
        // WHY: Check cancellation AFTER delivery and logging. If turn was
        // cancelled during tool execution, result is still delivered (agent's
        // responsibility to check turn state), but syscall returns E_CANCELLED
        // to inform caller that turn is no longer active.
        //
        // TRADE-OFF: Caller may waste work if turn was cancelled, but result
        // is preserved in history and delivered to agent for potential use.
        if cancelled {
            return Err(KernelError::cancelled("turn cancelled"));
        }

        // =====================================================================
        // PHASE 5: Acknowledgment
        // =====================================================================
        // WHY: Return Frame::ok to caller confirming result was delivered.
        // "delivered" flag distinguishes from "sent" (chat:tool uses "sent").
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"delivered": true})))
            .await;
        Ok(())
    }
}
