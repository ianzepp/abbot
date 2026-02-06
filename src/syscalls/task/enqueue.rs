//! Task:Enqueue - Add work items to scope-specific task queues
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables "head" agents to delegate work to "hand" agents by enqueuing tasks
//! into scope-specific FIFO queues. Tasks are organized by scope (e.g., project name, agent ID)
//! with round-robin fairness across scopes during lease operations.
//!
//! **Lane assignment: Task lane**
//! - WHY: Mutates shared `TaskKernel` state (queues, rr_scopes, active map)
//! - Serialization prevents race conditions when multiple agents enqueue concurrently
//! - Task lane ensures queue mutations are atomic (no torn writes)
//!
//! **Integration points:**
//! - `TaskKernel::enqueue()` - Adds task to scope queue, updates round-robin, notifies leasers
//! - `Kernel::bump_activity()` - Signals kernel that work is available (prevents shutdown)
//! - Frame protocol: Returns `task_id` on success
//!
//! **Coordination:**
//! - Enqueue never blocks (O(1) queue insertion)
//! - Wakes one waiting leaser via Notify (if any hand agents are blocked on `task:lease`)
//! - Scope added to round-robin queue if newly non-empty
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Non-blocking delegation**: Head agents never wait for hand availability
//! - **Scope-based fairness**: Round-robin prevents head-of-line blocking across scopes
//! - **Fire-and-forget**: Enqueue returns immediately; head can poll via `task:status`
//! - **Flexible scopes**: Any string can be a scope (project name, agent ID, etc.)
//! - **Auto-generated IDs**: Task ID optional (defaults to UUID if not provided)
//!
//! CONCURRENCY
//! ===========
//! - Task lane serialization ensures queue mutations are atomic
//! - Multiple heads can enqueue concurrently (serialized by lane)
//! - Notify::notify_one() wakes exactly one waiting leaser per enqueued task
//! - Activity bump prevents kernel shutdown while tasks are pending

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TaskKernel};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for enqueuing tasks into scope-specific queues.
///
/// WHY: Encapsulates task creation and queue insertion. Delegates to `TaskKernel`
/// for actual queue management, keeping syscall layer thin.
pub struct TaskEnqueue;

impl TaskEnqueue {
    /// Create a new `TaskEnqueue` syscall.
    ///
    /// WHY: Standard constructor for syscall registration.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskEnqueue {
    fn name(&self) -> &'static str {
        "task:enqueue"
    }

    /// Enqueue a task for execution by a hand agent.
    ///
    /// WHY: Enables "head" agents to delegate work to "hand" agents via scope-based
    /// queues. Tasks are enqueued with FIFO ordering within scope and round-robin
    /// fairness across scopes during lease operations.
    ///
    /// USE CASE: Head agent breaks project into tasks, enqueues each with scope=project_id.
    /// Hand agents call `task:lease` to claim tasks in round-robin order across projects.
    ///
    /// ARGUMENTS:
    /// - `goal` (required): Task objective/description
    /// - `task_id` (optional): Custom task ID (defaults to UUID)
    /// - `head_id` (optional): ID of enqueuing agent (defaults to "unknown")
    /// - `scope` (optional): Queue scope for round-robin (defaults to "main")
    /// - `input` (optional): Additional input data (defaults to goal text)
    /// - `notify_scope` (optional): Scope to notify on completion (for pub/sub)
    /// - `reply_to` (optional): UUID for request-response pattern
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"task_id": "..."}` on success
    /// - `E_INVALID_ARGS` if goal is missing or malformed
    /// - `E_INTERNAL` if kernel not initialized
    ///
    /// COORDINATION:
    /// - Task inserted into `TaskKernel::queues[scope]` (FIFO per scope)
    /// - Scope added to `TaskKernel::rr_scopes` if newly non-empty
    /// - Task status set to `Queued` in `TaskKernel::active` map
    /// - One waiting leaser woken via Notify (if any)
    /// - Activity bumped to prevent kernel shutdown
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // WHY: Early cancellation check prevents wasted work on cancelled operations.
        ctx.check_cancelled()?;

        // WHY: Kernel::get() retrieves global kernel instance for TaskKernel access.
        // Returns E_INTERNAL if kernel not initialized (should never happen in practice).
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // WHY: TaskKernel::task_from_json() validates arguments and constructs TaskItem.
        // Provides clear error messages for missing/malformed fields (especially "goal").
        let task = TaskKernel::task_from_json(data).map_err(KernelError::invalid_args)?;
        let task_id = task.id.clone();

        // WHY: TaskKernel::enqueue() handles queue insertion, round-robin update, and
        // leaser notification. Async because it acquires multiple locks (queues, rr_scopes, active).
        k.tasks().enqueue(task).await;

        // WHY: Activity bump signals kernel that work is pending. Without this, kernel
        // might shutdown thinking it's idle while tasks are queued.
        k.bump_activity();

        // WHY: Return task_id to caller so they can poll status via `task:status` or
        // wait for completion notification via `reply_to` mechanism.
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"task_id": task_id})))
            .await;
        Ok(())
    }
}
