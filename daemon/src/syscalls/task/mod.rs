//! Task Namespace - Scope-based task queue and execution coordination
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `task` namespace implements a **scope-based task queue system** with round-robin fairness
//! that enables "head" agents to delegate work to "hand" agents for execution. Unlike the `need`
//! namespace (priority queue), tasks are organized into scope-specific queues with fair scheduling
//! across scopes.
//!
//! **Core architecture:**
//! - `TaskKernel` maintains three data structures:
//!   - `queues`: HashMap of scope -> VecDeque<TaskItem> (FIFO per scope)
//!   - `rr_scopes`: VecDeque<String> for round-robin scope selection
//!   - `active`: HashMap tracking task status (Queued/Running/Done)
//!   - `watchers`: HashMap of task_id -> Notify for status change notifications
//! - `Notify` primitive enables efficient blocking when all queues are empty
//!
//! **Integration points:**
//! - `task:enqueue` - Add work items to a scope-specific queue (any agent)
//! - `task:lease` - Claim next task using round-robin fairness (blocks if empty)
//! - `task:complete` - Mark leased task as complete with success/failure status
//! - `task:status` - Query current state of a task (Queued/Running/Done)
//! - `task:read` - Retrieve execution log from SQLite history (hand_execs table)
//! - `task:list` - List tasks with status filtering (placeholder implementation)
//! - `task:search` - Search tasks by pattern (placeholder implementation)
//!
//! **Lane assignment:**
//! - MOST task syscalls execute on the **Task lane** for serialized queue mutations
//! - EXCEPTION: `task:lease` runs on **Immediate lane** to prevent deadlock
//!   - WHY: If lease shared the Task lane with enqueue, lease would hold the lane lock
//!     while blocking (waiting for tasks), preventing enqueue from adding tasks → deadlock
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Scope-based fairness**: Round-robin across scopes prevents head-of-line blocking
//! - **Decoupled delegation**: "Head" agents enqueue work; "hand" agents lease and execute
//! - **Async coordination**: Lease blocks efficiently (not polling) until work is available
//! - **Status tracking**: Tasks transition Queued → Running → Done with notifications
//! - **Non-blocking enqueue**: Adding tasks never blocks (O(1) queue insertion)
//! - **FIFO within scope**: Tasks within the same scope execute in enqueue order
//! - **Persistent execution log**: Hand agent tool calls saved to SQLite for auditing
//!
//! TASK LIFECYCLE
//! ==============
//! 1. **Enqueue** - "Head" agent calls `task:enqueue` to delegate work to a "hand" agent
//!    - Task enters scope-specific queue (`queues[scope]`)
//!    - Status set to `Queued` in `active` map
//!    - Scope added to `rr_scopes` if newly non-empty
//!    - Notify wakes one waiting leaser (if any)
//!    - Activity bump signals kernel that work is available
//!
//! 2. **Lease** - "Hand" agent calls `task:lease` to claim next task
//!    - Blocks until at least one queue is non-empty (via Notify)
//!    - Round-robin selection: Try each scope in `rr_scopes` queue
//!    - Pop task from selected scope (FIFO within scope)
//!    - Status updated to `Running { hand_id, started_at }` in `active` map
//!    - Task watcher notified of status change
//!    - Returns task details (id, head_id, scope, prompt, input, reply_to)
//!
//! 3. **Execute** - "Hand" agent executes the task (tool calls, LLM invocations)
//!    - Tool calls logged to SQLite `hand_execs` table via history::Store
//!    - Execution is NOT part of task syscalls (happens in hand agent runtime)
//!
//! 4. **Complete** - "Hand" agent calls `task:complete` after finishing work
//!    - Status updated to `Done { ok, summary, finished_at }` in `active` map
//!    - Task watcher notified (allows polling via `task:status`)
//!    - Activity bump signals kernel that task finished
//!    - Task remains in `active` map (no automatic cleanup)
//!
//! WHY SCOPE-BASED QUEUES?
//! =======================
//! Without scopes, a single FIFO queue creates head-of-line blocking:
//! - Scenario: 1000 tasks for project A, 1 urgent task for project B
//! - Single queue: Project B task waits behind all 1000 project A tasks
//! - Scope-based queues with round-robin: Project B task executes after ≤1 project A task
//!
//! Round-robin scheduling ensures fairness across different work streams (projects, agents,
//! or conceptual groupings) while maintaining FIFO within each scope.
//!
//! ROUND-ROBIN MECHANICS
//! ======================
//! The `rr_scopes` queue tracks which scopes have pending work:
//! 1. Lease pops scope from front of `rr_scopes`
//! 2. Lease pops task from that scope's queue
//! 3. If scope still has tasks, push scope to back of `rr_scopes`
//! 4. Otherwise, scope removed from `rr_scopes` (no pending work)
//!
//! Example:
//! - Scopes: ["project-a", "project-b", "project-c"]
//! - Each has 3 tasks enqueued
//! - Lease 1: "project-a" (rr_scopes → ["project-b", "project-c", "project-a"])
//! - Lease 2: "project-b" (rr_scopes → ["project-c", "project-a", "project-b"])
//! - Lease 3: "project-c" (rr_scopes → ["project-a", "project-b", "project-c"])
//! - Lease 4: "project-a" again (fair rotation)
//!
//! TASK STATUS STATES
//! ==================
//! - **Queued**: Task enqueued but not yet leased (waiting for hand agent)
//! - **Running { hand_id, started_at }**: Task leased by hand agent (in progress)
//! - **Done { ok, summary, finished_at }**: Task completed (success or failure)
//!
//! Status transitions are one-way: Queued → Running → Done. Tasks are never re-queued
//! (no automatic retry). If a hand agent crashes, task remains in Running state (orphaned).
//!
//! USE CASES
//! =========
//! 1. **Project execution**: Head agent breaks project into tasks, hands execute in parallel
//! 2. **Multi-agent coordination**: Multiple hands claim tasks from shared scope
//! 3. **Request-response**: Head enqueues task with `reply_to` UUID, polls via `task:status`
//! 4. **Execution auditing**: Retrieve tool call logs via `task:read` for debugging
//! 5. **Fair scheduling**: Round-robin ensures no scope monopolizes hand agent capacity
//!
//! CONCURRENCY
//! ===========
//! - Multiple agents can call `task:enqueue` concurrently (queue/rr_scopes are Mutex-protected)
//! - Multiple hand agents can call `task:lease` concurrently (all block until tasks available)
//! - Notify::notify_one() wakes exactly one waiting leaser per enqueued task
//! - Round-robin selection is atomic (lock held for duration of scope selection + pop)
//! - Task lane serialization prevents race conditions on queue/status mutations
//! - EXCEPTION: `task:lease` runs on Immediate lane to avoid deadlock with enqueue
//!
//! LANE COORDINATION
//! =================
//! **Task Lane (serialized execution):**
//! - `task:enqueue` - Mutates `queues`, `rr_scopes`, `active` (requires serialization)
//! - `task:complete` - Mutates `active` status map (requires serialization)
//! - `task:status` - Reads `active` map (serialized to avoid torn reads)
//! - `task:list`, `task:read`, `task:search` - No shared state access (placeholder/SQLite)
//!
//! **Immediate Lane (concurrent execution):**
//! - `task:lease` - WHY: Long-polling operation that blocks waiting for work
//!   - If lease ran on Task lane, it would hold the lane lock while blocked
//!   - This would prevent enqueue from adding tasks → deadlock
//!   - Immediate lane allows lease to block without starving enqueue
//!
//! TRADE-OFFS
//! ==========
//! 1. **No Automatic Cleanup**
//!    - CHOSEN: Completed tasks remain in `active` map indefinitely
//!    - REJECTED: Automatic removal after completion
//!    - WHY: Enables status polling after completion (head agent can check result)
//!    - IMPLICATION: Memory grows unbounded with completed tasks (requires manual GC)
//!
//! 2. **No Task Timeout/Expiration**
//!    - CHOSEN: Tasks remain in Running state indefinitely once leased
//!    - REJECTED: Automatic re-enqueue after lease timeout
//!    - WHY: Simpler implementation, hand agents responsible for timely completion
//!    - IMPLICATION: Crashed hand agents leave orphaned tasks in Running state
//!
//! 3. **No Priority Levels**
//!    - CHOSEN: FIFO within scope, round-robin across scopes (fairness)
//!    - REJECTED: Priority queue like `need` namespace
//!    - WHY: Task delegation focuses on fairness, not urgency
//!    - IMPLICATION: Cannot express "this task is more urgent" (use `need` for that)
//!
//! 4. **No Batching**
//!    - CHOSEN: Lease returns single task, not batch of N tasks
//!    - WHY: Simpler API, hand agents can call lease in loop if batching desired
//!    - IMPLICATION: High-throughput scenarios have per-task overhead (mutex lock/unlock)
//!
//! 5. **In-Memory Queue (No Persistence)**
//!    - CHOSEN: Task queues stored only in memory
//!    - REJECTED: SQLite persistence for crash recovery
//!    - WHY: Tasks are ephemeral work coordination, not durable state
//!    - IMPLICATION: Kernel restart loses all queued and running tasks
//!    - NOTE: Execution logs ARE persisted to SQLite (hand_execs table)
//!
//! PERSISTENCE MODEL
//! =================
//! - **Queues (NOT persisted)**: `queues`, `rr_scopes`, `active`, `watchers` are in-memory
//! - **Execution logs (persisted)**: Hand agent tool calls saved to SQLite `hand_execs` table
//! - **Rationale**: Queue state is transient coordination; execution history is audit trail
//!
//! COMPARISON: TASK VS. NEED
//! ==========================
//! | Feature                  | `task:*`                | `need:*`                  |
//! |--------------------------|-------------------------|---------------------------|
//! | Scheduling               | FIFO + round-robin      | Priority queue            |
//! | Organization             | Scope-based queues      | Single global queue       |
//! | Fairness                 | Round-robin across scopes | Priority-based preemption |
//! | Status tracking          | Queued/Running/Done     | Queued/Running            |
//! | Completion notification  | Yes (via watchers)      | Yes (via fulfillment)     |
//! | Use case                 | Delegated work items    | Urgent coordination       |
//! | Lane assignment          | Task (except lease)     | Need (except lease)       |

mod complete;
mod enqueue;
mod lease;
mod list;
mod read;
mod search;
mod status;

pub use complete::TaskComplete;
pub use enqueue::TaskEnqueue;
pub use lease::TaskLease;
pub use list::TaskList;
pub use read::TaskRead;
pub use search::TaskSearch;
pub use status::TaskStatusGet;

/// Register all task syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration for the task namespace. Called during
/// kernel initialization to make task coordination syscalls available to all agents.
///
/// REGISTERED SYSCALLS:
/// - `task:enqueue` - Add work item to scope-specific queue (Task lane)
/// - `task:lease` - Claim next task via round-robin (Immediate lane to prevent deadlock)
/// - `task:complete` - Mark leased task as complete with status (Task lane)
/// - `task:status` - Query current state of a task (Task lane)
/// - `task:list` - List tasks with filtering (Task lane, placeholder)
/// - `task:read` - Retrieve execution log from SQLite (Task lane)
/// - `task:search` - Search tasks by pattern (Task lane, placeholder)
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(TaskEnqueue::new()));
    dispatcher.register(Arc::new(TaskLease::new()));
    dispatcher.register(Arc::new(TaskComplete::new()));
    dispatcher.register(Arc::new(TaskStatusGet::new()));
    dispatcher.register(Arc::new(TaskList::new()));
    dispatcher.register(Arc::new(TaskRead::new()));
    dispatcher.register(Arc::new(TaskSearch::new()));
}
