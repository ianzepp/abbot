//! Task:Complete - Mark leased tasks as finished with success/failure status
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables "hand" agents to mark tasks as complete after execution, transitioning
//! task status from `Running` to `Done`. Includes success flag and summary message for auditing.
//!
//! **Lane assignment: Task lane**
//! - WHY: Mutates shared `TaskKernel::active` status map
//! - Serialization prevents race conditions on status updates
//! - Task lane ensures status transitions are atomic
//!
//! **Integration points:**
//! - `TaskKernel::complete()` - Updates status to Done, notifies watchers
//! - `Kernel::bump_activity()` - Signals kernel that task finished
//! - Frame protocol: Returns `{"updated": true}` on success
//!
//! **Coordination:**
//! - Status updated to `Done { ok, summary, finished_at }` in `active` map
//! - Task watcher notified (allows polling via `task:status`)
//! - Task remains in `active` map (no automatic cleanup)
//! - Activity bump signals kernel that work finished
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Explicit completion**: Hand agent must explicitly mark task as done (no auto-cleanup)
//! - **Success/failure tracking**: Boolean flag + summary message for auditing
//! - **Persistent status**: Task remains in `active` map for post-completion queries
//! - **Watcher notification**: Enables polling/waiting on task completion
//! - **Idempotent**: Calling complete multiple times updates status each time (last write wins)
//!
//! CONCURRENCY
//! ===========
//! - Task lane serialization ensures status updates are atomic
//! - Multiple calls to complete for same task_id serialized (last write wins)
//! - Watcher notification wakes all waiters on task_id (allows multiple pollers)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for marking tasks as complete with success/failure status.
///
/// WHY: Encapsulates task completion logic. Delegates to `TaskKernel` for status
/// update and watcher notification, keeping syscall layer thin.
pub struct TaskComplete;

impl TaskComplete {
    /// Create a new `TaskComplete` syscall.
    ///
    /// WHY: Standard constructor for syscall registration.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskComplete {
    fn name(&self) -> &'static str {
        "task:complete"
    }

    /// Mark a leased task as complete with success/failure status.
    ///
    /// WHY: Enables "hand" agents to signal task completion after execution. Transitions
    /// task status from `Running` to `Done`, allowing "head" agents to poll completion
    /// via `task:status` or receive notifications via watchers.
    ///
    /// USE CASE: Hand agent leases task, executes tool calls, then marks complete with
    /// success=true and summary of actions taken. Head agent polls status to detect completion.
    ///
    /// ARGUMENTS:
    /// - `task_id` (required): Unique task identifier from lease
    /// - `ok` (optional): Success flag (defaults to false)
    /// - `summary` (optional): Human-readable completion message (defaults to "")
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"updated": true}` on success
    /// - `E_INVALID_ARGS` if task_id is missing or empty
    /// - `E_INTERNAL` if kernel not initialized
    ///
    /// STATUS TRANSITION:
    /// - `Running { hand_id, started_at }` → `Done { ok, summary, finished_at }`
    /// - Task remains in `active` map (no automatic cleanup)
    /// - Previous status overwritten (idempotent, last write wins)
    ///
    /// COORDINATION:
    /// - Task watcher notified via Notify::notify_waiters() (wakes all pollers)
    /// - Activity bumped to signal kernel that work finished
    /// - No validation that task exists or is in Running state (best-effort update)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // WHY: Early cancellation check prevents wasted work on cancelled operations.
        ctx.check_cancelled()?;

        // WHY: Kernel::get() retrieves global kernel instance for TaskKernel access.
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // WHY: Extract and validate task_id. Empty task_id rejected because it's
        // meaningless for status updates.
        let task_id = data
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err(KernelError::invalid_args("task_id is required"));
        }

        // WHY: Extract success flag (defaults to false for safety). Empty summary
        // allowed (hand agent may not provide details).
        let ok = data.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let summary = data
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // WHY: TaskKernel::complete() updates status to Done and notifies watchers.
        // No validation that task exists or is Running (best-effort update).
        // Async because it acquires multiple locks (active, watchers).
        k.tasks().complete(task_id, ok, summary).await;

        // WHY: Activity bump signals kernel that work finished. Without this, kernel
        // might think it's idle when tasks are completing.
        k.bump_activity();

        // WHY: Return success confirmation. updated=true indicates status was updated
        // (even if task didn't exist - idempotent operation).
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"updated": true})))
            .await;
        Ok(())
    }
}
