//! Tool:DeliverResult - Backward-compatible alias for tool:result
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall is a backward-compatible alias for tool:result. It provides the
//! same functionality but uses the old syscall name for compatibility with older
//! external executors.
//!
//! **Deprecation notice:**
//! - NEW CODE: Use tool:result (preferred name)
//! - OLD CODE: tool:deliver_result continues to work (this syscall)
//! - WHY: Shorter, clearer name aligns with tool:register, tool:explain
//!
//! **Implementation:**
//! - Delegates to shared deliver_result() function in mod.rs
//! - Identical behavior to tool:result (no functional difference)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Backward compatibility**: Old external executors continue to work
//! - **Deprecation path**: New code should use tool:result
//! - **No duplication**: Both syscalls share the same implementation
//!
//! TRADE-OFFS
//! ==========
//! 1. **Two syscall names for one operation**
//!    - CHOSEN: Keep both tool:result and tool:deliver_result
//!    - WHY: Backward compatibility with external executors
//!    - IMPLICATION: Documentation must clarify preferred name

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::deliver_result;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Backward-compatible alias for tool:result.
///
/// WHY: Provides compatibility with older external executors that use the old
/// syscall name. New code should use tool:result instead.
///
/// DEPRECATION: Prefer tool:result in new code. This syscall continues to work
/// for backward compatibility but may be removed in a future version.
pub struct ToolDeliverResult;

impl Default for ToolDeliverResult {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolDeliverResult {
    /// Create a new ToolDeliverResult syscall.
    ///
    /// WHY: Standard constructor pattern for syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolDeliverResult {
    fn name(&self) -> &'static str {
        "tool:deliver_result"
    }

    /// Deliver external tool execution result to waiting agent.
    ///
    /// WHY: Backward-compatible alias for tool:result. Enables older external
    /// executors to deliver results using the old syscall name.
    ///
    /// DEPRECATION: New code should use tool:result instead. This syscall name
    /// is maintained for backward compatibility only.
    ///
    /// USE CASE:
    /// - Older external executor delivers result using tool:deliver_result
    /// - Syscall delegates to same implementation as tool:result
    /// - No functional difference (identical behavior)
    ///
    /// See tool:result documentation for detailed usage information.
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
