//! Need:Enqueue - Add work items to the priority queue
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables agents to add work items (needs) to the kernel's priority queue
//! for asynchronous processing by other agents. Needs represent tasks, requests, or
//! coordination messages that are prioritized by urgency level and processed in FIFO
//! order within each priority tier.
//!
//! **Core operation:**
//! 1. Parse and validate need arguments from JSON (need_id, need, priority, etc.)
//! 2. Insert need into `NeedKernel` priority queue (BinaryHeap)
//! 3. Wake one waiting leaser via `tokio::sync::Notify`
//! 4. Bump kernel activity counter (prevents idle shutdown)
//! 5. Return success acknowledgment
//!
//! **Integration points:**
//! - `NeedKernel::need_from_json()` for argument validation and NeedItem construction
//! - `NeedKernel::enqueue()` for priority queue insertion
//! - `Kernel::bump_activity()` to signal active work
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{"enqueued": true}` on success
//! - Returns `E_INVALID_ARGS` for missing/malformed need arguments
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Non-blocking**: Enqueue never waits (O(log n) heap insertion)
//! - **Fail-fast validation**: Invalid arguments rejected before queue modification
//! - **Priority-aware**: Urgent needs preempt normal/low priority work
//! - **Universal access**: All agent types may enqueue needs (no actor restrictions)
//! - **FIFO within priority**: Equal-priority needs processed in creation order
//!
//! CONCURRENCY
//! ===========
//! - Multiple agents may enqueue needs concurrently (NeedKernel uses Mutex internally)
//! - Enqueue operation is atomic (either inserts or fails, no partial state)
//! - Notify::notify_one() wakes exactly one waiting leaser per enqueued need
//! - Safe for concurrent execution across all task lanes
//!
//! NEED ARGUMENTS
//! ==============
//! Required fields:
//! - `need_id` (string): Unique identifier for this need (used for fulfillment tracking)
//! - `need` (string): Description of work to be performed
//!
//! Optional fields:
//! - `priority` (string): "urgent", "high", "normal" (default), or "low"
//! - `source` (string): Agent/component that created this need (default: "unknown")
//! - `scope` (string): Logical grouping/namespace for need (default: "main")
//! - `context` (string): Additional context or metadata for processing
//! - `reply_to` (UUID string): Call ID for request-response coordination
//! - `reconvene` (bool): Whether to trigger room reconvening after fulfillment
//!
//! WHY NO ACTOR RESTRICTIONS?
//! ===========================
//! Unlike mutating syscalls (proc:run, git:run), `need:enqueue` does NOT require
//! mutation permission via `ctx.require_mutation()`. This is intentional:
//!
//! - Enqueuing a need does not mutate filesystem or git state (safe for all agents)
//! - "Hand" agents (LLM-controlled) need to enqueue work items for "head" agents
//! - Room coordination requires bidirectional need creation (head <-> hand)
//!
//! SECURITY: The actual work execution happens in `need:lease` handlers, which
//! can implement their own authorization checks based on need contents.
//!
//! PRIORITY LEVELS
//! ===============
//! - **Urgent** (3): Critical operations that preempt all other work
//!   - Example: User-requested cancellation, crash recovery
//! - **High** (2): Important but not critical
//!   - Example: User queries, test failures
//! - **Normal** (1): Default priority for routine work
//!   - Example: Code analysis, documentation generation
//! - **Low** (0): Background tasks, lowest priority
//!   - Example: Cache warming, log rotation
//!
//! PERFORMANCE
//! ===========
//! - Enqueue is O(log n) due to BinaryHeap insertion
//! - No persistence overhead (in-memory only)
//! - Notify wakes exactly one waiter (no thundering herd)
//!
//! TRADE-OFFS
//! ==========
//! 1. **In-Memory Only**
//!    - CHOSEN: Needs not persisted to SQLite
//!    - WHY: Needs are ephemeral coordination primitives (like HTTP requests)
//!    - IMPLICATION: Kernel restart loses all queued needs (acceptable for dev tool)
//!
//! 2. **No Duplicate Detection**
//!    - CHOSEN: Multiple needs can have same need_id if enqueued separately
//!    - REJECTED: Check for duplicate need_id before enqueue
//!    - WHY: Duplicate checking requires O(n) scan or separate index (expensive)
//!    - IMPLICATION: Callers responsible for deduplication (e.g., via reply_to UUID)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, NeedKernel, Syscall, SyscallContext};
use crate::runtime::Kernel;

/// Syscall for enqueueing work items to the need priority queue.
///
/// WHY: Enables asynchronous task coordination between agents without direct coupling.
/// Agents enqueue needs describing work to be done, and other agents lease them for processing.
pub struct NeedEnqueue;

impl NeedEnqueue {
    /// Create a new `NeedEnqueue` syscall.
    ///
    /// WHY: Zero-state constructor (syscall is stateless, all state lives in NeedKernel).
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedEnqueue {
    fn name(&self) -> &'static str {
        "need:enqueue"
    }

    /// Enqueue a work item to the priority queue.
    ///
    /// WHY: Primary mechanism for agents to coordinate asynchronous work. Enables
    /// decoupled communication where producers enqueue needs and consumers lease them.
    ///
    /// USE CASE: Invoked by all agent types to create work items:
    /// - "Head" agents enqueue needs for "hand" agents to execute
    /// - "Hand" agents enqueue needs for "head" agents (e.g., request approval)
    /// - Room coordinators enqueue needs for participant agents
    ///
    /// ARGUMENTS:
    /// - `need_id` (string, required): Unique identifier for fulfillment tracking
    /// - `need` (string, required): Description of work to perform
    /// - `priority` (string, optional): "urgent"|"high"|"normal"|"low" (default: "normal")
    /// - `source` (string, optional): Agent/component creating need (default: "unknown")
    /// - `scope` (string, optional): Logical grouping (default: "main")
    /// - `context` (string, optional): Additional metadata
    /// - `reply_to` (UUID, optional): Call ID for request-response coordination
    /// - `reconvene` (bool, optional): Trigger room reconvening after fulfillment
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"enqueued": true}` on success
    /// - `E_INVALID_ARGS` if need_id or need fields missing/empty
    /// - `E_INTERNAL` if kernel not initialized (should never happen in production)
    ///
    /// CONCURRENCY NOTE: Multiple agents may enqueue concurrently. NeedKernel uses
    /// internal Mutex to serialize queue modifications, so this operation is atomic.
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // VALIDATION
        // =====================================================================
        // WHY: Check cancellation before expensive operations
        ctx.check_cancelled()?;

        // WHY: Kernel access required for NeedKernel. Should always be present
        // in production (kernel initialized before syscalls registered).
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // =====================================================================
        // ARGUMENT PARSING
        // =====================================================================
        // WHY: need_from_json validates required fields (need_id, need) and
        // provides sensible defaults for optional fields (priority=normal, etc.)
        let need = NeedKernel::need_from_json(data).map_err(|e| KernelError::invalid_args(e))?;

        // =====================================================================
        // ENQUEUE & NOTIFICATION
        // =====================================================================
        // WHY: Insert into priority queue and wake one waiting leaser.
        // This is the core operation - O(log n) heap insertion.
        k.needs().enqueue(need).await;

        // WHY: Signal kernel activity to prevent idle shutdown. Enqueuing work
        // indicates the system is actively coordinating tasks.
        k.bump_activity();

        // =====================================================================
        // RESPONSE
        // =====================================================================
        // WHY: Acknowledge successful enqueue. Simple boolean response since
        // enqueue never fails after validation (always succeeds).
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"enqueued": true})))
            .await;
        Ok(())
    }
}
