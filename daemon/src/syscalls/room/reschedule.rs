//! Room:Reschedule - Update run_after_ms for pending schedule
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall updates the `run_after_ms` field of a room schedule, enabling deferred
//! execution or retry with exponential backoff. Only affects schedules with status
//! "pending" (running/done/cancelled schedules cannot be rescheduled).
//!
//! **Use cases:**
//! - Exponential backoff: Reschedule failed room with increasing delay (e.g., 1min,
//!   2min, 4min, 8min) to avoid thundering herd on transient errors
//! - Deferred execution: Push scheduled room further into future if conditions aren't
//!   met (e.g., waiting for external service to become available)
//! - Priority adjustment: Reschedule low-priority room to later time if high-priority
//!   rooms are pending (manual load balancing)
//!
//! **Status constraints:**
//! - Only "pending" schedules can be rescheduled (enforced by Store layer)
//! - WHY: Running rooms cannot be time-traveled (already executing). Done/cancelled
//!   rooms are historical records (should not be mutated).
//! - WORKAROUND: To retry failed room, query via room:list, create new schedule with
//!   incremented attempts counter
//!
//! **Integration points:**
//! - `Store::reschedule_room_schedule()` - Updates `run_after_ms` in `room_schedules` table
//! - Returns `updated: bool` indicating whether schedule was found and modified
//! - RoomCoordinator polls schedules on each tick, executes when `run_after_ms` <= now
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Time-only modification**: Only `run_after_ms` is mutable (not room_type, room,
//!   or constraints). This simplifies logic and prevents accidentally changing room
//!   configuration during rescheduling.
//! - **Idempotent updates**: Rescheduling with same `run_after_ms` is no-op (doesn't
//!   increment attempts counter or change status).
//! - **Explicit timing**: Caller must compute new `run_after_ms` (no relative offsets
//!   like "+1hour"). This makes retry logic explicit and testable.
//!
//! CONCURRENCY
//! ===========
//! - Safe for concurrent execution (SQLite handles write serialization)
//! - Multiple callers can reschedule different schedules simultaneously
//! - Rescheduling same schedule concurrently results in last-write-wins (acceptable
//!   since retry logic is best-effort, not transactional)
//!
//! SECURITY MODEL
//! ==============
//! - No permission checks (any actor can reschedule rooms)
//! - WHY permissive: Rescheduling only changes execution time, not room configuration.
//!   Malicious rescheduling (e.g., moving all rooms to distant future) is detectable
//!   via room:list and reversible via additional reschedule calls.
//!
//! TRADE-OFFS
//! ==========
//! 1. **Time-only vs full update**: Only `run_after_ms` is mutable. This prevents
//!    accidentally changing room type or room during rescheduling but means retrying
//!    with different configuration requires creating new schedule.
//!
//! 2. **Absolute time vs relative offset**: Caller provides absolute Unix timestamp
//!    (not relative like "+1hour"). This makes retry logic explicit but requires
//!    caller to compute timestamp (no server-side convenience functions).

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for updating run_after_ms of pending room schedules.
///
/// WHY: Enables deferred execution and retry with exponential backoff. Only
/// modifies execution time (not room configuration) for simplicity.
pub struct RoomReschedule;

impl Default for RoomReschedule {
    fn default() -> Self {
        Self::new()
    }
}

impl RoomReschedule {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomReschedule {
    fn name(&self) -> &'static str {
        "room:reschedule"
    }

    /// Update run_after_ms for a pending room schedule.
    ///
    /// WHY this exists: Enables retry logic with exponential backoff (failed rooms
    /// can be rescheduled with increasing delay) and deferred execution (push room
    /// to later time if conditions aren't met).
    ///
    /// ARGUMENTS:
    /// - `id` or `schedule_id`: UUID of schedule to reschedule
    /// - `run_after_ms`: New execution time (Unix timestamp ms)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{id, updated, run_after_ms}` on success
    /// - `updated: true` if schedule was found and modified
    /// - `updated: false` if schedule not found or not in "pending" status
    /// - `E_INVALID_ARGS` if id or run_after_ms missing
    /// - `E_INTERNAL` if kernel store not attached or update fails
    ///
    /// CONSTRAINTS:
    /// - Only "pending" schedules can be rescheduled (enforced by Store layer)
    /// - Running/done/cancelled schedules return `updated: false`
    ///
    /// USAGE:
    /// ```json
    /// {"id": "550e8400-e29b-41d4-a716-446655440000", "run_after_ms": 1704067200000}
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

        // WHY accept both "id" and "schedule_id": Backward compatibility with existing
        // callers that may use either field name. "id" is preferred for brevity.
        let id = data
            .get("id")
            .or_else(|| data.get("schedule_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::invalid_args("id is required"))?;

        let run_after_ms = data
            .get("run_after_ms")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| KernelError::invalid_args("run_after_ms is required"))?;

        // WHY Store::reschedule_room_schedule: Updates `run_after_ms` in
        // `room_schedules` table. Only affects schedules with status "pending"
        // (returns updated=false for running/done/cancelled schedules).
        let updated = store
            .reschedule_room_schedule(id, run_after_ms)
            .await
            .map_err(|e| KernelError::internal(format!("failed to reschedule: {}", e)))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"id": id, "updated": updated, "run_after_ms": run_after_ms}),
            ))
            .await;
        Ok(())
    }
}
