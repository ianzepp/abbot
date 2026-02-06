//! Room:List - Query room schedules with optional filtering
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall queries the `room_schedules` SQLite table with optional filtering
//! by status, room_type, and limit. It enables observability into scheduled rooms
//! (what's pending, what ran, what failed) and supports building UIs that display
//! room execution history.
//!
//! **Query capabilities:**
//! - Filter by status: "pending", "running", "done", "cancelled", "failed"
//! - Filter by room_type: "conclave", "autonomy", "work"
//! - Limit results: Default 50, configurable up to query limit
//! - Results ordered by created_at_ms descending (most recent first)
//!
//! **Schedule fields returned:**
//! - `id`: Schedule UUID (for use in room:reschedule, room:cancel)
//! - `room_type`, `scope`: Room configuration
//! - `status`: Current execution state
//! - `run_after_ms`: When room should execute (Unix timestamp ms)
//! - `reason`: Why room was scheduled (e.g., "slow idle", "nightly reflection")
//! - `wake_mode`: "normal" or "init"
//! - `context`: Optional scheduling context
//! - `attempts`: Retry count (incremented on failure + reschedule)
//! - `last_error`: Error message from most recent failed execution
//! - `created_at_ms`: When schedule was created (Unix timestamp ms)
//!
//! **Use cases:**
//! - Debugging: See which rooms are pending/running/failed
//! - Monitoring: Build dashboard showing room execution statistics
//! - Retry logic: Query failed schedules to implement exponential backoff
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Optional filtering**: All filters are optional so callers can query "all
//!   schedules" (no filter), "pending schedules" (status filter), or "conclave
//!   schedules" (room_type filter) with the same syscall.
//! - **Bounded results**: Default limit of 50 prevents unbounded memory usage when
//!   querying large schedule tables (long-running daemons may accumulate thousands
//!   of historical schedules).
//! - **Full metadata exposure**: Returns all schedule fields (including attempts,
//!   last_error) so callers can implement retry logic or debugging UIs without
//!   additional queries.
//!
//! CONCURRENCY
//! ===========
//! - Read-only query (safe for concurrent execution)
//! - Multiple callers can list schedules simultaneously without coordination
//! - SQLite handles read concurrency (no explicit locking required)
//!
//! SECURITY MODEL
//! ==============
//! - No permission checks (any actor can list schedules)
//! - WHY permissive: Room schedules are internal coordination metadata. Observability
//!   is critical for debugging but doesn't grant control over execution.
//! - All schedules are visible to all actors (no per-room access control)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for querying room schedules from SQLite with optional filtering.
///
/// WHY: Enables observability into scheduled rooms (pending, running, failed)
/// and supports building UIs that display room execution history.
pub struct RoomList;

impl RoomList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomList {
    fn name(&self) -> &'static str {
        "room:list"
    }

    /// Query room schedules with optional filtering.
    ///
    /// WHY this exists: Provides observability into room scheduling and execution.
    /// Callers can query pending rooms (for UI display), failed rooms (for retry
    /// logic), or historical rooms (for analysis of deliberation frequency).
    ///
    /// ARGUMENTS:
    /// - `status`: Optional filter ("pending", "running", "done", "cancelled", "failed")
    /// - `room_type` or `type`: Optional filter ("conclave", "autonomy", "work")
    /// - `limit`: Maximum results to return (default: 50)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{schedules: [...], count: N}` on success
    /// - Each schedule includes: id, room_type, scope, status, run_after_ms, reason,
    ///   wake_mode, context, attempts, last_error, created_at_ms
    /// - `E_INTERNAL` if kernel store not attached or query fails
    ///
    /// USAGE:
    /// ```json
    /// {"status": "pending", "limit": 10}
    /// ```
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        let status = data.get("status").and_then(|v| v.as_str());
        let room_type = data
            .get("room_type")
            .or_else(|| data.get("type"))
            .and_then(|v| v.as_str());

        // WHY default limit of 50: Prevents unbounded memory usage when querying
        // large schedule tables. Long-running daemons may accumulate thousands of
        // historical schedules (one per idle detection trigger).
        let limit = data
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;

        // WHY Store::list_room_schedules: Queries `room_schedules` table with
        // optional WHERE clauses for status and room_type. Results ordered by
        // created_at_ms descending (most recent first).
        let schedules = store
            .list_room_schedules(status, room_type, limit)
            .map_err(|e| KernelError::internal(format!("failed to list schedules: {}", e)))?;

        // WHY return all fields: Enables callers to implement retry logic (check
        // attempts, last_error), display execution history (status, created_at_ms),
        // or build monitoring dashboards (room_type distribution) without additional
        // queries.
        let items: Vec<serde_json::Value> = schedules
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "room_type": s.room_type,
                    "scope": s.scope,
                    "status": s.status,
                    "run_after_ms": s.run_after_ms,
                    "reason": s.reason,
                    "wake_mode": s.wake_mode,
                    "context": s.context,
                    "attempts": s.attempts,
                    "last_error": s.last_error,
                    "created_at_ms": s.created_at_ms,
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"schedules": items, "count": items.len()}),
            ))
            .await;
        Ok(())
    }
}
