//! Chat:Tool - Register external tool calls initiated by agents
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall registers tool calls initiated by agents (typically LLM-generated via hand agents)
//! and broadcasts them to subscribers for external execution. It implements the **request side**
//! of the tool coordination protocol: agent requests tool → chat:tool registers + broadcasts →
//! external executor runs tool → chat:tool_result delivers result back to agent.
//!
//! **Critical design decisions:**
//! - Tool calls are "external" (executed outside the agent's immediate context)
//! - TurnTracker registers the tool call before broadcasting (prevents duplicate tool_result)
//! - Sigcalls broadcasts enable real-time UI updates (show user what agent is doing)
//! - No logging at registration time (only when tool_result is delivered)
//!
//! **Integration points:**
//! - `TurnTracker.register_external_tool()` - Registers tool call ID for result delivery
//! - `Sigcalls` - Real-time broadcast to subscribers (UI shows "Agent is running X tool")
//!
//! **Frame protocol:**
//! - Emits `Frame::item` (type: tool_call) via Sigcalls with tool_call_id, name, arguments
//! - Returns `Frame::ok` to caller with `{"sent": true}` acknowledgment
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Register-before-broadcast**: TurnTracker registration prevents race conditions
//! - **No actor restrictions**: Any actor may register tool calls (head, hand, room)
//! - **Argument validation**: Arguments must be JSON object (prevents malformed tool calls)
//! - **Signal-first**: Real-time broadcast to subscribers (no logging until tool_result)
//!
//! CONCURRENCY
//! ===========
//! - **Lane assignment**: Immediate lane (tool calls are part of user-facing conversation)
//! - **TurnTracker access**: Thread-safe (Arc<Mutex<...>>), registration is atomic
//! - **Sigcalls broadcast**: Async send (no blocking)
//!
//! TOOL COORDINATION LIFECYCLE
//! ===========================
//! WHY separate chat:tool and chat:tool_result syscalls:
//!
//! 1. **Agent initiates tool call** (chat:tool):
//!    - TurnTracker.register_external_tool() records tool_call_id + name
//!    - Sigcalls broadcasts tool_call event to subscribers
//!    - External executor (UI, daemon, etc.) receives notification
//!
//! 2. **External executor runs tool** (outside kernel):
//!    - Executor receives tool_call_id, name, arguments via Sigcalls subscription
//!    - Executor runs tool (e.g., read file, run git command, etc.)
//!    - Executor captures result (success or error)
//!
//! 3. **Executor delivers result** (chat:tool_result):
//!    - TurnTracker.deliver_external_tool_result() matches tool_call_id
//!    - Agent receives result and continues processing
//!    - Tool result is logged to history
//!
//! WHY this split:
//! - Decouples tool execution from agent context (tool may be slow, async)
//! - Enables external executors (tools run outside kernel sandbox)
//! - Supports cancellation (turn can be cancelled while tool is running)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TurnKey};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_scope};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for registering external tool calls initiated by agents.
///
/// WHY: Zero-sized struct (stateless). All coordination logic is in execute().
pub struct ChatTool;

#[async_trait]
impl Syscall for ChatTool {
    fn name(&self) -> &'static str {
        "chat:tool"
    }

    /// Register an external tool call initiated by an agent.
    ///
    /// WHY: Enables agents to request tool execution outside their immediate context.
    /// External executors (UI, daemon, etc.) subscribe to Sigcalls and run tools,
    /// then deliver results via chat:tool_result.
    ///
    /// USE CASE:
    /// - Agent (hand) emits LLM tool call: `{"tool_call_id": "call_abc123", "name": "read_file", "arguments": {"path": "auth.rs"}}`
    /// - chat:tool registers in TurnTracker and broadcasts to subscribers
    /// - External executor receives notification and runs read_file tool
    /// - Executor calls chat:tool_result with result
    ///
    /// SECURITY NOTE: No actor restrictions (any agent may register tools). Tool
    /// execution security is enforced by external executor (e.g., VFS path validation).
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"sent": true}` on successful registration + broadcast
    /// - `E_INVALID_ARGS` if tool_call_id, name, or arguments are malformed
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
        // WHY: Validate all required fields before TurnTracker registration.
        // Early failure prevents invalid tool calls from being registered.
        ctx.check_cancelled()?;

        let scope = parse_scope(&data)?;
        let reply_to = parse_reply_to(&data)?;

        // WHY: tool_call_id is LLM-generated (or manually specified). Must be
        // unique within the turn to enable result delivery matching.
        let tool_call_id = data
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if tool_call_id.is_empty() {
            return Err(KernelError::invalid_args("tool_call_id is required"));
        }

        // WHY: Tool name identifies the operation to execute (e.g., "read_file",
        // "git_status"). External executor dispatches based on this name.
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            return Err(KernelError::invalid_args("name is required"));
        }

        // WHY: Arguments must be a JSON object (enforces structured tool calls).
        // Prevents malformed arguments like strings or arrays.
        let arguments = data.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
        if !arguments.is_object() {
            return Err(KernelError::invalid_args("arguments must be an object"));
        }

        // =====================================================================
        // PHASE 2: TurnTracker Registration
        // =====================================================================
        // WHY: Register tool call BEFORE broadcasting to prevent race condition:
        // - If broadcast-first: external executor might deliver result before registration (result lost)
        // - If register-first: result delivery waits for registration (guaranteed delivery)
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let key = TurnKey::new(scope, reply_to);

        // WHY: TurnTracker.register_external_tool() validates:
        // - tool_call_id is unique within turn (no duplicate registrations)
        // - Turn exists and is not cancelled
        // Returns error if registration fails (duplicate ID, cancelled turn, etc.)
        k.turns()
            .register_external_tool(&key, tool_call_id, name)
            .await
            .map_err(KernelError::invalid_args)?;

        // =====================================================================
        // PHASE 3: Signal Broadcasting
        // =====================================================================
        // WHY: Broadcast Frame::item with type "tool_call" to subscribers.
        // External executors receive this and dispatch tool execution.
        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::item(
                    ctx.call_id,
                    json!({
                        "type": "tool_call",
                        "tool_call_id": tool_call_id,
                        "name": name,
                        "arguments": arguments,
                    }),
                )
                .with_name("chat:tool")
                .with_actor(ctx.actor_str().to_string()),
            )
            .await;

        // =====================================================================
        // PHASE 4: Acknowledgment
        // =====================================================================
        // WHY: Return Frame::ok to caller confirming tool call was registered
        // and broadcast. Does NOT wait for tool execution or result delivery.
        let _ = tx.send(Frame::ok(ctx.call_id, json!({"sent": true}))).await;
        Ok(())
    }
}
