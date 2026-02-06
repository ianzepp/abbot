//! Need:Lease - Claim next highest-priority need from the queue
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables agents to claim (lease) the next highest-priority work item from
//! the need queue for processing. The lease operation **blocks** until a need becomes
//! available, making it the primary synchronization point for need-based coordination.
//!
//! **Core operation:**
//! 1. Wait for queue to become non-empty (via `tokio::sync::Notify`)
//! 2. Pop highest-priority need from BinaryHeap
//! 3. Insert need into `active` HashMap (lease tracking)
//! 4. Return need details to caller
//! 5. Bump kernel activity counter
//!
//! **Blocking semantics:**
//! - Lease **blocks indefinitely** if queue is empty (async wait via Notify)
//! - Respects context cancellation (tokio::select! enables early exit)
//! - Multiple agents can block on lease concurrently (all wait on same Notify)
//!
//! **Integration points:**
//! - `NeedKernel::lease()` for blocking queue pop + active tracking
//! - `SyscallContext::cancel` for cancellation propagation
//! - `Kernel::bump_activity()` to signal active work
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with need details on successful lease
//! - Returns `E_CANCELLED` if context cancelled during wait
//! - Returns `E_INTERNAL` if kernel not initialized
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Blocking coordination**: Lease blocks until work available (no polling/busy-wait)
//! - **Priority-aware**: Always returns highest-priority need first
//! - **Lease tracking**: Active HashMap prevents duplicate processing
//! - **Cancellation-friendly**: Respects context cancellation during wait
//! - **Universal access**: All agent types may lease needs (no actor restrictions)
//! - **FIFO within priority**: Equal-priority needs returned in creation order
//!
//! CONCURRENCY
//! ===========
//! - Multiple agents may call `need:lease` concurrently (all block until needs available)
//! - Each enqueued need wakes exactly one waiting leaser (Notify::notify_one)
//! - Lease operation is atomic (either returns need or cancels, no partial state)
//! - Active HashMap prevents same need being leased twice
//! - Safe for concurrent execution across all task lanes
//!
//! LEASE SEMANTICS
//! ===============
//! When an agent leases a need, it is **responsible** for:
//! 1. Processing the work described in the need
//! 2. Calling `need:fulfill` with the need_id to complete the lease
//! 3. Handling errors gracefully (still call fulfill on failure to release lease)
//!
//! **WHY LEASE TRACKING?**
//! Without the `active` HashMap, multiple agents could process the same need:
//! - Agent A calls lease() -> receives Need #1, starts processing
//! - Agent B calls lease() before A finishes -> would receive Need #1 again (BAD!)
//!
//! The `active` map prevents this by marking leased needs as "in progress". Only
//! `need:fulfill` removes a need from active, ensuring exactly-once processing
//! (unless the processing agent crashes without fulfilling).
//!
//! **ORPHANED LEASES:**
//! If an agent crashes after leasing but before fulfilling, the need remains in
//! `active` indefinitely. This is acceptable for a development tool (manual cleanup
//! via kernel restart). A production system would implement lease timeouts or
//! heartbeat-based expiration.
//!
//! WHY NO TIMEOUT ARGUMENT?
//! ========================
//! Unlike most blocking syscalls, `need:lease` does NOT accept a timeout argument.
//! This is intentional:
//!
//! - Lease represents "wait for work to arrive" semantics (like queue consumer)
//! - Callers who need timeouts can use `tokio::time::timeout()` wrapper
//! - Context cancellation provides escape hatch for shutdown/abort
//!
//! ALTERNATIVE DESIGN: Add `timeout_ms` argument that returns E_TIMEOUT if no need
//! available within duration. Rejected for simplicity - callers can implement timeout
//! logic externally if needed.
//!
//! PRIORITY LEVELS
//! ===============
//! Lease always returns the highest-priority need first:
//! - **Urgent** (3): Critical operations that preempt all other work
//! - **High** (2): Important but not critical
//! - **Normal** (1): Default priority for routine work
//! - **Low** (0): Background tasks, lowest priority
//!
//! Within the same priority level, needs are FIFO (oldest first). This is implemented
//! via `NeedItem::Ord`, which sorts by `(priority desc, created_at asc)`.
//!
//! PERFORMANCE
//! ===========
//! - Lease is O(log n) due to BinaryHeap pop
//! - Blocking wait is efficient (Notify uses futex/wait primitives, no spinning)
//! - No thundering herd (notify_one wakes exactly one waiter per enqueued need)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Indefinite Blocking**
//!    - CHOSEN: Lease blocks until need available or context cancelled
//!    - REJECTED: Timeout-based lease that returns "no work" after duration
//!    - WHY: Simpler API, aligns with queue consumer semantics
//!    - IMPLICATION: Callers must implement timeout logic externally if needed
//!
//! 2. **No Batch Leasing**
//!    - CHOSEN: Lease returns single need, not batch of N needs
//!    - WHY: Simpler implementation, agents can call lease in loop for batching
//!    - IMPLICATION: High-throughput scenarios may prefer batch API (future enhancement)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

/// Syscall for claiming the next highest-priority need from the queue.
///
/// WHY: Provides blocking work-claiming semantics for need-based coordination.
/// Agents call lease to wait for work, process it, then call fulfill to complete.
pub struct NeedLease;

impl NeedLease {
    /// Create a new `NeedLease` syscall.
    ///
    /// WHY: Zero-state constructor (syscall is stateless, all state lives in NeedKernel).
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedLease {
    fn name(&self) -> &'static str {
        "need:lease"
    }

    /// Claim the next highest-priority need from the queue.
    ///
    /// WHY: Primary synchronization point for need-based coordination. Agents block
    /// on lease until work becomes available, then process the need and call fulfill.
    ///
    /// USE CASE: Invoked by worker agents in a loop to continuously process needs:
    /// ```rust
    /// loop {
    ///     let need = need:lease().await?;  // Block until work available
    ///     process_need(&need).await?;
    ///     need:fulfill(need.id).await?;    // Release lease
    /// }
    /// ```
    ///
    /// BLOCKING SEMANTICS:
    /// - Blocks **indefinitely** until a need becomes available
    /// - Respects context cancellation (returns E_CANCELLED if cancelled during wait)
    /// - Multiple agents can block on lease concurrently
    /// - Each enqueued need wakes exactly one waiting leaser
    ///
    /// ARGUMENTS:
    /// - No arguments (lease always returns next highest-priority need)
    ///
    /// RETURNS:
    /// - `Frame::ok` with need details:
    ///   - `need_id`: Unique identifier (use for fulfillment)
    ///   - `source`: Agent/component that created this need
    ///   - `priority`: "urgent"|"high"|"normal"|"low"
    ///   - `need`: Description of work to perform
    ///   - `context`: Additional metadata
    ///   - `scope`: Logical grouping
    ///   - `reply_to`: Optional call ID for request-response
    ///   - `reconvene`: Whether to trigger room reconvening after fulfillment
    /// - `E_CANCELLED` if context cancelled during wait
    /// - `E_INTERNAL` if kernel not initialized
    ///
    /// LEASE RESPONSIBILITY:
    /// After receiving a need, the agent MUST call `need:fulfill` with the need_id
    /// to release the lease (even on error). Failing to fulfill leaves the need
    /// orphaned in the `active` set, preventing re-processing.
    ///
    /// CONCURRENCY NOTE: Multiple agents may lease concurrently. Each enqueued need
    /// wakes exactly one waiter (Notify::notify_one), so N needs will wake N agents.
    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // VALIDATION
        // =====================================================================
        // WHY: Check cancellation before blocking wait. Prevents waiting for
        // work when parent task is already cancelled.
        ctx.check_cancelled()?;

        // WHY: Kernel access required for NeedKernel. Should always be present
        // in production (kernel initialized before syscalls registered).
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // =====================================================================
        // BLOCKING LEASE WITH CANCELLATION SUPPORT
        // =====================================================================
        // WHY: tokio::select! enables cancellation during blocking wait.
        // Without this, lease would block indefinitely even if context cancelled.
        //
        // HOW IT WORKS:
        // - If ctx.cancel fires first -> return E_CANCELLED immediately
        // - If k.needs().lease() completes first -> continue with need processing
        //
        // CONCURRENCY: lease() blocks on Notify::notified() until queue non-empty.
        // This is an efficient async wait (no polling/spinning).
        let need = tokio::select! {
            _ = ctx.cancel.cancelled() => {
                return Err(KernelError::cancelled("operation cancelled"));
            }
            n = k.needs().lease() => n,
        };

        // WHY: Signal kernel activity to prevent idle shutdown. Leasing work
        // indicates the system is actively processing tasks.
        k.bump_activity();

        // =====================================================================
        // RESPONSE FORMATTING
        // =====================================================================
        // WHY: Return all need fields to caller for processing. Priority is
        // formatted as lowercase string ("urgent", "high", etc.) for JSON readability.
        //
        // NOTE: reply_to is optional (None if not a request-response need), so
        // we map it to Option<String> for JSON serialization.
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "need_id": need.id,
                    "source": need.source,
                    "priority": format!("{:?}", need.priority).to_ascii_lowercase(),
                    "need": need.need,
                    "context": need.context,
                    "scope": need.scope,
                    "reply_to": need.reply_to.map(|u| u.to_string()),
                    "reconvene": need.reconvene,
                }),
            ))
            .await;
        Ok(())
    }
}
