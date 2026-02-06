//! Task:Status - Query current state of tasks (Queued/Running/Done)
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables agents to query the current status of tasks in the task coordination
//! system. Returns Queued/Running/Done state with relevant metadata (hand_id for Running,
//! ok/summary for Done).
//!
//! **Lane assignment: Task lane**
//! - WHY: Reads from shared `TaskKernel::active` status map
//! - Serialization prevents torn reads (reading while status is being updated)
//! - Task lane ensures consistent status snapshots
//!
//! **Integration points:**
//! - `TaskKernel::status()` - Reads from active map, returns Option<TaskStatus>
//! - Frame protocol: Returns status details or `{"exists": false}`
//!
//! **Coordination:**
//! - Non-blocking read (no waiting/polling built into this syscall)
//! - For waiting on completion, use `TaskKernel::watcher()` + Notify::notified()
//! - Returns None if task never existed or hasn't been enqueued yet
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Snapshot query**: Returns current status at call time (no blocking)
//! - **Exists flag**: Distinguishes "task not found" from "task queued"
//! - **Rich metadata**: Includes hand_id for Running, ok/summary for Done
//! - **Read-only**: No side effects, safe to call repeatedly
//! - **Polling-friendly**: Head agents can poll in loop for completion detection
//!
//! STATUS STATES
//! =============
//! - **Queued**: Task enqueued but not yet leased
//! - **Running { hand_id, started_at }**: Task leased by hand agent (in progress)
//! - **Done { ok, summary, finished_at }**: Task completed (success or failure)
//! - **None**: Task ID not found in active map (never enqueued or cleaned up)
//!
//! CONCURRENCY
//! ===========
//! - Task lane serialization ensures consistent status reads
//! - Multiple agents can query same task_id concurrently (serialized reads)
//! - No modification of status map (read-only operation)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TaskStatus};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for querying task status.
///
/// WHY: Encapsulates status query logic. Delegates to `TaskKernel` for actual
/// status lookup, keeping syscall layer thin.
pub struct TaskStatusGet;

impl TaskStatusGet {
    /// Create a new `TaskStatusGet` syscall.
    ///
    /// WHY: Standard constructor for syscall registration.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskStatusGet {
    fn name(&self) -> &'static str {
        "task:status"
    }

    /// Query the current status of a task.
    ///
    /// WHY: Enables agents to check task progress without blocking. Used by "head"
    /// agents to poll for completion after enqueuing work, or to verify tasks are
    /// being processed by "hand" agents.
    ///
    /// USE CASE: Head agent enqueues task, then polls status in loop until Done.
    /// Alternatively, head agent can use `TaskKernel::watcher()` + Notify for
    /// event-driven waiting (not exposed as syscall).
    ///
    /// ARGUMENTS:
    /// - `task_id` (required): Unique task identifier from enqueue
    ///
    /// RETURNS:
    /// - `Frame::ok` with status details:
    ///   - `{"exists": false}` if task not found
    ///   - `{"exists": true, "status": "queued"}` if task is queued
    ///   - `{"exists": true, "status": "running", "hand_id": "..."}` if task is running
    ///   - `{"exists": true, "status": "done", "ok": bool, "summary": "..."}` if task is done
    /// - `E_INVALID_ARGS` if task_id is missing or empty
    /// - `E_INTERNAL` if kernel not initialized
    ///
    /// NON-BLOCKING:
    /// - Returns immediately with current status (no waiting)
    /// - For blocking wait, combine with tokio::select! + TaskKernel::watcher()
    ///
    /// COORDINATION:
    /// - Reads from `TaskKernel::active` map (serialized by Task lane)
    /// - No side effects (safe to call repeatedly)
    /// - Returns snapshot at call time (status may change immediately after)
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
        // meaningless for status queries.
        let task_id = data
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err(KernelError::invalid_args("task_id is required"));
        }

        // WHY: TaskKernel::status() reads from active map. Returns None if task not found,
        // otherwise returns current status. Async because it acquires lock on active map.
        let status = k.tasks().status(task_id).await;

        // WHY: Convert TaskStatus enum to JSON object with exists flag. This allows
        // callers to distinguish "task not found" from "task queued" (both would be
        // falsy without exists flag).
        let out = match status {
            // WHY: None means task never enqueued (or cleaned up after completion).
            // exists=false signals to caller that task_id is invalid.
            None => json!({"exists": false}),

            // WHY: Queued means task in queue waiting for hand agent to lease.
            // No additional metadata needed (just status string).
            Some(TaskStatus::Queued) => json!({"exists": true, "status": "queued"}),

            // WHY: Running means task leased by hand agent. Include hand_id for
            // auditing (which hand agent is processing this task).
            // started_at not included (for simplicity, can be added if needed).
            Some(TaskStatus::Running { hand_id, .. }) => {
                json!({"exists": true, "status": "running", "hand_id": hand_id})
            }

            // WHY: Done means task completed. Include ok flag (success/failure) and
            // summary message for auditing. finished_at not included (can be added if needed).
            Some(TaskStatus::Done { ok, summary, .. }) => {
                json!({"exists": true, "status": "done", "ok": ok, "summary": summary})
            }
        };

        // WHY: Return status details to caller. Always succeeds (E_NOT_FOUND not used,
        // exists=false is sufficient).
        let _ = tx.send(Frame::ok(ctx.call_id, out)).await;
        Ok(())
    }
}
