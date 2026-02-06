//! Task:Lease - Claim tasks from scope-based queues with round-robin fairness
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables "hand" agents to claim tasks from scope-based queues using round-robin
//! fairness. Lease blocks efficiently (via tokio::sync::Notify) when all queues are empty,
//! waking immediately when a task is enqueued.
//!
//! **Lane assignment: IMMEDIATE (not Task lane)**
//! - WHY: Lease is a long-polling operation that blocks waiting for work
//! - If lease ran on Task lane (shared with enqueue), deadlock occurs:
//!   1. Lease acquires Task lane lock
//!   2. Lease blocks waiting for task (still holding lock)
//!   3. Enqueue tries to acquire Task lane lock → waits forever
//!   4. No tasks ever arrive → deadlock
//! - Immediate lane allows lease to block without starving enqueue operations
//!
//! **Integration points:**
//! - `TaskKernel::lease()` - Round-robin task selection, blocks if empty
//! - `tokio::select!` - Enables cancellation during blocking wait
//! - `Kernel::bump_activity()` - Signals kernel that work is being processed
//!
//! **Coordination:**
//! - Blocks until at least one queue is non-empty (via Notify)
//! - Round-robin selection ensures fairness across scopes
//! - Status updated to `Running { hand_id, started_at }` on successful lease
//! - Task watcher notified (allows polling via `task:status`)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Efficient blocking**: Notify-based waiting (not polling) minimizes CPU usage
//! - **Cancellation hygiene**: Respects parent cancellation during blocking wait
//! - **Round-robin fairness**: Prevents head-of-line blocking across scopes
//! - **Immediate lane execution**: Prevents deadlock with enqueue operations
//! - **Status tracking**: Leased task marked as Running with hand_id for auditing
//!
//! ROUND-ROBIN MECHANICS
//! =====================
//! 1. Pop scope from front of `rr_scopes` queue
//! 2. Pop task from that scope's FIFO queue
//! 3. If scope still has tasks, push scope to back of `rr_scopes`
//! 4. Otherwise, scope removed from `rr_scopes` (empty)
//!
//! Example with 3 scopes (A, B, C) each having tasks:
//! - Lease 1: A (rr_scopes → [B, C, A])
//! - Lease 2: B (rr_scopes → [C, A, B])
//! - Lease 3: C (rr_scopes → [A, B, C])
//! - Lease 4: A again (fair rotation)
//!
//! CONCURRENCY
//! ===========
//! - Multiple hand agents can call lease concurrently (all block if empty)
//! - Notify::notify_one() wakes exactly one leaser per enqueued task
//! - Round-robin selection is atomic (lock held for entire scope selection + task pop)
//! - Immediate lane execution allows concurrent lease calls without blocking enqueue

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for leasing tasks from scope-based queues with round-robin fairness.
///
/// WHY: Encapsulates blocking wait and round-robin selection logic. Delegates to
/// `TaskKernel` for actual queue management, keeping syscall layer thin.
pub struct TaskLease;

impl TaskLease {
    /// Create a new `TaskLease` syscall.
    ///
    /// WHY: Standard constructor for syscall registration.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskLease {
    fn name(&self) -> &'static str {
        "task:lease"
    }

    /// Claim the next task from scope-based queues using round-robin fairness.
    ///
    /// WHY: Enables "hand" agents to claim work items delegated by "head" agents.
    /// Blocks efficiently (via Notify) when all queues are empty, waking immediately
    /// when a task is enqueued. Round-robin ensures fairness across scopes.
    ///
    /// USE CASE: Hand agent calls lease in a loop, executing each claimed task until
    /// shutdown. Multiple hand agents can lease concurrently for parallel execution.
    ///
    /// ARGUMENTS:
    /// - `hand_id` (optional): ID of claiming agent (defaults to "hand")
    ///
    /// RETURNS:
    /// - `Frame::ok` with task details:
    ///   - `task_id`: Unique task identifier
    ///   - `head_id`: ID of agent that enqueued the task
    ///   - `scope`: Scope (queue) from which task was selected
    ///   - `goal`: Task objective/description
    ///   - `input`: Additional input data for execution
    ///   - `notify_scope`: Optional scope to notify on completion
    ///   - `reply_to`: Optional UUID for request-response pattern
    /// - `E_CANCELLED` if parent context cancels during blocking wait
    /// - `E_INVALID_ARGS` if hand_id is empty
    /// - `E_INTERNAL` if kernel not initialized
    ///
    /// BLOCKING BEHAVIOR:
    /// - Blocks indefinitely until a task becomes available (or cancellation)
    /// - Wakes immediately when `task:enqueue` adds a task and calls notify_one()
    /// - Respects parent cancellation via tokio::select! (returns E_CANCELLED)
    ///
    /// COORDINATION:
    /// - Round-robin selection across scopes (prevents head-of-line blocking)
    /// - Task status updated to `Running { hand_id, started_at }` on lease
    /// - Task watcher notified (allows `task:status` polling)
    /// - Activity bumped to signal kernel that work is being processed
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // WHY: Early cancellation check prevents initiating lease if already cancelled.
        ctx.check_cancelled()?;

        // WHY: Kernel::get() retrieves global kernel instance for TaskKernel access.
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // WHY: Extract hand_id for status tracking. Defaults to "hand" if not provided.
        // Empty hand_id rejected because it's meaningless for auditing.
        let hand_id = data
            .get("hand_id")
            .and_then(|v| v.as_str())
            .unwrap_or("hand")
            .trim();
        if hand_id.is_empty() {
            return Err(KernelError::invalid_args("hand_id is required"));
        }

        // WHY: tokio::select! enables cancellation during blocking wait. Without this,
        // lease would ignore parent cancellation until a task becomes available.
        //
        // CONCURRENCY: TaskKernel::lease() blocks via Notify when all queues empty.
        // Notify::notify_one() (called by enqueue) wakes exactly one waiting leaser.
        let task = tokio::select! {
            // WHY: Cancellation branch returns E_CANCELLED immediately, preventing
            // hand agent from processing a task after parent context cancelled.
            _ = ctx.cancel.cancelled() => {
                return Err(KernelError::cancelled("operation cancelled"));
            }

            // WHY: Lease branch blocks until task available, then atomically pops task
            // from round-robin selected scope and updates status to Running.
            t = k.tasks().lease(hand_id) => t,
        };

        // WHY: Activity bump signals kernel that work is being processed. Without this,
        // kernel might think it's idle even though hand agent is executing a task.
        k.bump_activity();

        // WHY: Return full task details to hand agent so it can execute the work.
        // reply_to converted to string for JSON serialization (Option<Uuid> unsupported).
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "task_id": task.id,
                    "head_id": task.head_id,
                    "scope": task.scope,
                    "goal": task.goal,
                    "input": task.input,
                    "notify_scope": task.notify_scope,
                    "reply_to": task.reply_to.map(|u| u.to_string()),
                }),
            ))
            .await;
        Ok(())
    }
}
