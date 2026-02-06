//! Need Namespace - Asynchronous task coordination via priority queue
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `need` namespace implements a **priority queue-based task coordination system** that
//! enables asynchronous communication between agents in the Abbot kernel. Needs represent
//! work items that agents can enqueue, lease (claim for processing), and fulfill (mark complete).
//!
//! **Core architecture:**
//! - `NeedKernel` maintains two data structures:
//!   - `queue`: Priority queue of pending needs (BinaryHeap sorted by priority + timestamp)
//!   - `active`: HashMap of leased needs currently being processed
//! - `Notify` primitive enables efficient blocking when queue is empty
//!
//! **Integration points:**
//! - `need:enqueue` - Add work items to the queue (any agent)
//! - `need:lease` - Claim next highest-priority need (blocks if empty)
//! - `need:fulfill` - Mark leased need as complete, remove from active set
//!
//! **Lane assignment:**
//! - All need syscalls execute on the **Need lane** for prioritized task coordination
//! - Lease operation blocks until a need becomes available (async wait via Notify)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Decoupled coordination**: Agents communicate via needs without direct coupling
//! - **Priority-based scheduling**: Urgent needs preempt normal/low priority work
//! - **Lease semantics**: Active tracking prevents duplicate work on the same need
//! - **Non-blocking enqueue**: Adding needs never blocks (O(log n) heap insertion)
//! - **Efficient waiting**: Lease blocks efficiently via tokio::sync::Notify (no polling)
//! - **FIFO within priority**: Equal-priority needs are FIFO (uses creation timestamp)
//!
//! NEED LIFECYCLE
//! ==============
//! 1. **Enqueue** - Agent calls `need:enqueue` to add work item to priority queue
//!    - Need enters `queue` (BinaryHeap)
//!    - Notify wakes one waiting leaser (if any)
//!
//! 2. **Lease** - Agent calls `need:lease` to claim next need
//!    - Blocks until queue non-empty (via Notify)
//!    - Pops highest-priority need from `queue`
//!    - Inserts into `active` HashMap (need_id -> NeedItem)
//!    - Returns need details to leasing agent
//!
//! 3. **Fulfill** - Agent calls `need:fulfill` after completing work
//!    - Removes need from `active` HashMap
//!    - Returns whether need existed (allows idempotent fulfillment)
//!
//! WHY LEASE SEMANTICS?
//! ====================
//! Without lease tracking, two agents could process the same need concurrently:
//! - Agent A calls `need:lease` -> receives Need #1
//! - Agent B calls `need:lease` before A finishes -> would receive Need #1 again (if no lease)
//!
//! The `active` HashMap prevents this by marking leased needs as "in progress". Only
//! `need:fulfill` can remove a need from the active set, ensuring each need is processed
//! exactly once (unless the processing agent crashes without fulfilling).
//!
//! PRIORITY LEVELS
//! ===============
//! - **Urgent** (3): Critical operations that preempt all other work
//! - **High** (2): Important but not critical
//! - **Normal** (1): Default priority for routine work
//! - **Low** (0): Background tasks, lowest priority
//!
//! Within the same priority level, needs are FIFO (oldest first). This is implemented
//! via `NeedItem::Ord`, which sorts by `(priority desc, created_at asc)`.
//!
//! USE CASES
//! =========
//! 1. **Room coordination**: "Head" agent enqueues needs for "hand" agents to execute
//! 2. **Task delegation**: Long-running work split into multiple needs for parallel processing
//! 3. **Request-response**: Agent enqueues need with `reply_to` UUID, waits for response
//! 4. **Background jobs**: Low-priority needs for cleanup, optimization, etc.
//!
//! CONCURRENCY
//! ===========
//! - Multiple agents can call `need:enqueue` concurrently (queue is Mutex-protected)
//! - Multiple agents can call `need:lease` concurrently (all block until needs available)
//! - Notify::notify_one() wakes exactly one waiting leaser per enqueued need
//! - BinaryHeap ensures O(log n) insertion and O(log n) pop (efficient for high-throughput)
//!
//! TRADE-OFFS
//! ==========
//! 1. **In-Memory Only (No Persistence)**
//!    - CHOSEN: Needs stored only in memory (BinaryHeap + HashMap)
//!    - REJECTED: SQLite persistence for crash recovery
//!    - WHY: Needs are ephemeral coordination primitives, not durable state
//!    - IMPLICATION: Kernel restart loses all queued and active needs (acceptable for dev tool)
//!
//! 2. **No Timeout/Expiration**
//!    - CHOSEN: Leased needs remain in `active` set indefinitely
//!    - REJECTED: Automatic re-enqueue after lease timeout
//!    - WHY: Simpler implementation, agents responsible for timely fulfillment
//!    - IMPLICATION: Crashed agents leave orphaned needs in `active` set (requires manual cleanup)
//!
//! 3. **No Batching**
//!    - CHOSEN: Lease returns single need, not batch of N needs
//!    - WHY: Simpler API, agents can call lease in loop if batching desired
//!    - IMPLICATION: High-throughput scenarios may prefer batch API (future enhancement)

mod enqueue;
mod fulfill;
mod lease;

pub use enqueue::NeedEnqueue;
pub use fulfill::NeedFulfill;
pub use lease::NeedLease;

/// Register all need syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration for the need namespace. Called during
/// kernel initialization to make `need:enqueue`, `need:lease`, and `need:fulfill`
/// available to all agents.
///
/// REGISTERED SYSCALLS:
/// - `need:enqueue` - Add work item to priority queue
/// - `need:lease` - Claim next highest-priority need (blocks if empty)
/// - `need:fulfill` - Mark leased need as complete
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(NeedEnqueue::new()));
    dispatcher.register(Arc::new(NeedLease::new()));
    dispatcher.register(Arc::new(NeedFulfill::new()));
}
