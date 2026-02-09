//! Tool:Result - Deliver external tool execution result to waiting agent
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall delivers execution results from external tools back to waiting agents.
//! It completes the external tool execution lifecycle by routing results through
//! ExternalToolManager's oneshot channel mechanism.
//!
//! **Tool execution lifecycle:**
//! 1. Agent registers pending tool call (ExternalToolManager.register_pending)
//! 2. External executor receives tool call notification
//! 3. External executor runs tool outside kernel
//! 4. External executor calls tool:result with tool_call_id + output
//! 5. ExternalToolManager routes result to waiting agent via oneshot channel
//!
//! **Critical design decisions:**
//! - Idempotent delivery via recent_completed cache (256 entries)
//! - Oneshot channel for result routing (registered at pending time)
//! - No actor restrictions (any actor may deliver results)
//! - Result validation (tool_call_id must be registered as pending)
//!
//! **Integration points:**
//! - `ExternalToolManager.deliver_result()` - Routes result via oneshot channel
//! - External executors (UI, daemon, plugins) - Call this after tool execution
//! - Hand agents - Wait for result via oneshot receiver
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Idempotent delivery**: External executors may retry after reconnect
//! - **Validation**: Tool call must be registered as pending (prevents spurious results)
//! - **No actor restrictions**: Any actor may deliver results (trust external executors)
//! - **Oneshot channel**: Result delivery is one-time only (no broadcast)
//!
//! CONCURRENCY
//! ===========
//! - Result delivery via Mutex-protected pending map (atomic registration/delivery)
//! - recent_completed cache prevents duplicate delivery errors after retry
//! - Oneshot channel is consumed after delivery (no multiple deliveries)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Idempotency Cache Size**
//!    - CHOSEN: 256 recent completions
//!    - WHY: Balances memory usage vs retry window
//!    - IMPLICATION: Retries beyond 256 calls may error (acceptable for reconnect scenarios)
//!
//! 2. **Oneshot vs Broadcast**
//!    - CHOSEN: Oneshot channel (one result to one agent)
//!    - WHY: Tool execution is scoped to single agent context
//!    - IMPLICATION: Cannot broadcast result to multiple waiters

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::deliver_result;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for delivering external tool execution results to waiting agents.
///
/// WHY: Wraps shared deliver_result() implementation. This is the preferred
/// syscall name (tool:deliver_result is a backward-compatible alias).
pub struct ToolResult;

impl Default for ToolResult {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolResult {
    /// Create a new ToolResult syscall.
    ///
    /// WHY: Standard constructor pattern for syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolResult {
    fn name(&self) -> &'static str {
        "tool:result"
    }

    /// Deliver external tool execution result to waiting agent.
    ///
    /// WHY: Enables external executors to deliver tool results after execution.
    /// Completes the external tool execution lifecycle by routing results via
    /// ExternalToolManager's oneshot channel mechanism.
    ///
    /// USE CASE:
    /// - External executor receives tool call notification
    /// - Executor runs tool (e.g., read file, run git command)
    /// - Executor calls tool:result with tool_call_id + output
    /// - ExternalToolManager routes result to waiting agent
    ///
    /// IDEMPOTENCY: Result delivery is idempotent via recent_completed cache.
    /// If tool_call_id was recently completed, duplicate delivery succeeds silently.
    /// This prevents errors when external executor retries after reconnect.
    ///
    /// WHY idempotency: External executors may retry result delivery if they
    /// don't receive acknowledgment (e.g., after reconnect). We cache recent
    /// completions (256 entries) to allow retries without error.
    ///
    /// SECURITY NOTE: No actor restrictions (any actor may deliver results).
    /// Tool call must be registered as pending (prevents spurious result injection).
    ///
    /// ARGUMENTS:
    /// - `room` (string, required): Room identifier (e.g., "main")
    /// - `tool_call_id` (string, required): Tool call identifier from registration
    /// - `output` (string, optional): Tool execution result (default: empty string)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"delivered": true}` on success
    /// - `E_INVALID_ARGS` if room or tool_call_id is missing, or if tool_call_id not registered
    /// - `E_INTERNAL` if kernel is not initialized
    /// - `E_CANCELLED` if context is cancelled mid-execution
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // WHY: Delegate to shared deliver_result() implementation.
        // Both tool:result and tool:deliver_result use the same logic.
        deliver_result(ctx, data, tx).await
    }
}
