//! Need:Fulfill - Mark leased needs as complete and release lease
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall completes the need lifecycle by removing a leased need from the `active`
//! HashMap, signaling that processing is complete. Fulfill is the **only** way to release
//! a lease - leased needs remain in `active` indefinitely until fulfilled (or kernel restart).
//!
//! **Core operation:**
//! 1. Extract need_id from arguments
//! 2. Remove need from `active` HashMap in NeedKernel
//! 3. Return whether need existed (idempotent operation)
//! 4. Bump kernel activity counter
//!
//! **Integration points:**
//! - `NeedKernel::fulfill()` for active HashMap removal
//! - `Kernel::bump_activity()` to signal active work completion
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{"fulfilled": true/false}` indicating whether need existed
//! - Returns `E_INVALID_ARGS` if need_id missing or empty
//! - Returns `E_INTERNAL` if kernel not initialized
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Idempotent**: Fulfilling non-existent need_id succeeds (returns false)
//! - **Simple**: No validation beyond need_id presence (caller knows need context)
//! - **Non-blocking**: O(1) HashMap removal, no waiting
//! - **Universal access**: All agent types may fulfill needs (no actor restrictions)
//! - **Lease release**: Only way to remove need from active set (explicit completion)
//!
//! CONCURRENCY
//! ===========
//! - Multiple agents may call `need:fulfill` concurrently (NeedKernel uses Mutex internally)
//! - Fulfill operation is atomic (either removes or reports non-existence, no partial state)
//! - Safe for concurrent execution across all task lanes
//!
//! IDEMPOTENT SEMANTICS
//! ====================
//! Fulfill is intentionally idempotent to support retry scenarios:
//!
//! ```ignore
//! let need = need:lease().await?;
//! match process_need(&need).await {
//!     Ok(_) => need:fulfill(need.id).await?,  // Success path
//!     Err(e) => {
//!         log_error(e);
//!         need:fulfill(need.id).await?;        // Still release lease on error
//!     }
//! }
//! ```
//!
//! **WHY ALLOW DOUBLE-FULFILL?**
//! - Network retry: If first fulfill call times out, retry succeeds (returns false)
//! - Error handling: Agents can always call fulfill in error path without checking state
//! - Cleanup: Manual cleanup scripts can fulfill needs without "does it exist?" checks
//!
//! **RETURN VALUE:**
//! - `{"fulfilled": true}` - Need existed and was removed from active set
//! - `{"fulfilled": false}` - Need_id not found in active set (already fulfilled, never leased, or typo)
//!
//! WHY NO ACTOR RESTRICTIONS?
//! ===========================
//! Unlike mutating syscalls (proc:run, git:run), `need:fulfill` does NOT require
//! mutation permission via `ctx.require_mutation()`. This is intentional:
//!
//! - Fulfilling a need does not mutate filesystem or git state (safe for all agents)
//! - Any agent that leases a need should be able to fulfill it (symmetry)
//! - Room coordination requires agents to fulfill needs created by others
//!
//! **SECURITY CONSIDERATION:**
//! An agent could maliciously fulfill another agent's leased need, causing that agent's
//! fulfillment call to return false. However:
//! - This doesn't corrupt state (need is still processed, just double-fulfilled)
//! - Malicious agents have easier attack vectors (e.g., enqueue spam)
//! - Trusted agent assumption: All agents in kernel are cooperative (no adversarial scenarios)
//!
//! ALTERNATIVE DESIGN: Add lease token that must match to fulfill (prevents cross-agent
//! fulfillment). Rejected for simplicity - not needed in cooperative agent environment.
//!
//! ORPHANED LEASES
//! ===============
//! If an agent crashes after leasing but before fulfilling, the need remains in `active`
//! indefinitely (until kernel restart). This is acceptable for a development tool:
//!
//! - **Manual cleanup**: Kernel restart clears all needs (in-memory only)
//! - **Observable**: `NeedKernel::counts()` exposes active count for monitoring
//! - **Tolerable**: Orphaned leases don't block other work (queue still processable)
//!
//! **PRODUCTION ALTERNATIVE:**
//! A production system would implement lease timeouts:
//! - Store lease timestamp in NeedItem
//! - Background task periodically scans active set
//! - Re-enqueue needs with expired leases (e.g., lease_time + 5 minutes)
//!
//! PERFORMANCE
//! ===========
//! - Fulfill is O(1) due to HashMap removal by key
//! - No persistence overhead (in-memory only)
//! - No blocking or waiting (immediate return)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Idempotent vs. Error on Double-Fulfill**
//!    - CHOSEN: Idempotent (fulfilling non-existent need_id succeeds)
//!    - REJECTED: Return error if need_id not found in active set
//!    - WHY: Simplifies error handling, supports retries, enables cleanup scripts
//!    - IMPLICATION: Typos in need_id silently succeed (return false, no error)
//!
//! 2. **No Lease Token Validation**
//!    - CHOSEN: Any agent can fulfill any need_id
//!    - REJECTED: Only leasing agent can fulfill (via lease token)
//!    - WHY: Simpler API, sufficient for cooperative agent environment
//!    - IMPLICATION: Malicious agents could double-fulfill others' needs (tolerable)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

/// Syscall for fulfilling (completing) leased needs.
///
/// WHY: Releases leases by removing needs from the active set. This is the terminal
/// operation in the need lifecycle (enqueue -> lease -> fulfill).
pub struct NeedFulfill;

impl NeedFulfill {
    /// Create a new `NeedFulfill` syscall.
    ///
    /// WHY: Zero-state constructor (syscall is stateless, all state lives in NeedKernel).
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedFulfill {
    fn name(&self) -> &'static str {
        "need:fulfill"
    }

    /// Mark a leased need as complete and release its lease.
    ///
    /// WHY: Completes the need lifecycle by removing the need from the active set.
    /// This signals that processing is complete and prevents the need from being
    /// orphaned indefinitely in the active HashMap.
    ///
    /// USE CASE: Invoked by agents after processing a leased need:
    /// ```ignore
    /// let need = need:lease().await?;  // Claim work
    /// process_need(&need).await?;      // Do the work
    /// need:fulfill(need.id).await?;    // Release lease (ALWAYS call this)
    /// ```
    ///
    /// IDEMPOTENT SEMANTICS:
    /// - Fulfilling a need that exists in `active` removes it and returns true
    /// - Fulfilling a need that doesn't exist (already fulfilled, never leased, or typo) returns false
    /// - Both cases are considered success (no error)
    ///
    /// WHY IDEMPOTENT? Simplifies error handling:
    /// ```ignore
    /// match process_need(&need).await {
    ///     Ok(_) => need:fulfill(need.id).await?,   // Success path
    ///     Err(e) => {
    ///         log_error(e);
    ///         need:fulfill(need.id).await?;         // Still release lease (no "already fulfilled?" check)
    ///     }
    /// }
    /// ```
    ///
    /// ARGUMENTS:
    /// - `need_id` (string, required): Unique identifier from leased need
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"fulfilled": true}` if need existed and was removed
    /// - `Frame::ok` with `{"fulfilled": false}` if need_id not found in active set
    /// - `E_INVALID_ARGS` if need_id missing or empty
    /// - `E_INTERNAL` if kernel not initialized
    ///
    /// LEASE RESPONSIBILITY:
    /// Agents MUST call fulfill after leasing, even on error. Failing to fulfill leaves
    /// the need orphaned in the `active` set (memory leak until kernel restart).
    ///
    /// CONCURRENCY NOTE: Multiple agents may fulfill concurrently. If two agents somehow
    /// obtain the same need_id, only the first fulfill removes it (second returns false).
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // VALIDATION
        // =====================================================================
        // WHY: Check cancellation before accessing kernel state
        ctx.check_cancelled()?;

        // WHY: Kernel access required for NeedKernel. Should always be present
        // in production (kernel initialized before syscalls registered).
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // =====================================================================
        // ARGUMENT PARSING
        // =====================================================================
        // WHY: Extract and validate need_id. Trim whitespace to handle common
        // JSON formatting issues (trailing spaces, etc.)
        let need_id = data
            .get("need_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        // WHY: Fail fast on missing need_id. Empty string would never match
        // a real need (all needs have non-empty IDs), so reject early.
        if need_id.is_empty() {
            return Err(KernelError::invalid_args("need_id is required"));
        }

        // =====================================================================
        // FULFILLMENT
        // =====================================================================
        // WHY: Remove from active HashMap (O(1) operation). Returns Some(NeedItem)
        // if need existed, None if not found. We only care about existence (bool).
        let existed = k.needs().fulfill(need_id).await.is_some();

        // WHY: Signal kernel activity to prevent idle shutdown. Fulfilling work
        // indicates the system is actively completing tasks.
        k.bump_activity();

        // =====================================================================
        // RESPONSE
        // =====================================================================
        // WHY: Return whether need existed. This allows callers to detect typos
        // (fulfilled=false when expecting true) or confirm successful fulfillment.
        //
        // NOTE: Both true and false are successful operations (no error). Only
        // missing need_id returns error (E_INVALID_ARGS).
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"fulfilled": existed})))
            .await;
        Ok(())
    }
}
