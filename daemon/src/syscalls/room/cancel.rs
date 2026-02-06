//! Room:Cancel - Mark schedule as cancelled (prevents execution)
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall marks a room schedule as "cancelled" in the `room_schedules` table,
//! preventing RoomCoordinator from executing it. Only affects schedules with status
//! "pending" (running/done schedules cannot be cancelled retroactively).
//!
//! **Use cases:**
//! - Manual intervention: Cancel room that's no longer needed (e.g., idle trigger
//!   fired but user became active again before room ran)
//! - Policy enforcement: Cancel low-priority rooms when system is under load
//! - Debugging: Cancel problematic room type during investigation (e.g., cancel all
//!   work rooms if git worktree provisioning is failing)
//!
//! **Status transitions:**
//! - "pending" → "cancelled": Schedule marked as cancelled, won't execute
//! - "running": Cannot cancel (room already executing, would need separate cancellation
//!   mechanism for in-progress deliberation)
//! - "done": Cannot cancel (room already completed, status is historical record)
//!
//! **Cancellation vs deletion:**
//! - WHY mark cancelled rather than delete: Preserves schedule history for analysis
//!   (can query "how many rooms were cancelled vs executed")
//! - Cancelled schedules remain in table indefinitely (no automatic cleanup)
//! - WORKAROUND: If table grows too large, manually DELETE FROM room_schedules WHERE
//!   status='cancelled' AND created_at_ms < threshold
//!
//! **Integration points:**
//! - `Store::cancel_room_schedule()` - Updates status to "cancelled" in `room_schedules` table
//! - Returns `cancelled: bool` indicating whether schedule was found and modified
//! - RoomCoordinator skips schedules with status "cancelled" during tick polling
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Soft delete**: Mark as cancelled rather than hard delete so schedule history
//!   is preserved for analysis and debugging.
//! - **Idempotent cancellation**: Cancelling already-cancelled schedule is no-op
//!   (returns cancelled=true, doesn't error).
//! - **No cascading cancellation**: Cancelling schedule doesn't affect related rooms
//!   (e.g., cancelling conclave doesn't cancel subsequent autonomy rooms).
//!
//! CONCURRENCY
//! ===========
//! - Safe for concurrent execution (SQLite handles write serialization)
//! - Race condition: If RoomCoordinator starts executing schedule between cancel
//!   call and status update, room may run despite cancellation request. Acceptable
//!   because cancellation is best-effort, not transactional.
//!
//! SECURITY MODEL
//! ==============
//! - No permission checks (any actor can cancel rooms)
//! - WHY permissive: Cancellation is reversible (reschedule with new run_after_ms).
//!   Malicious cancellation (e.g., cancel all rooms) is detectable via room:list
//!   and reversible via room:schedule.
//!
//! TRADE-OFFS
//! ==========
//! 1. **Soft delete vs hard delete**: Cancelled schedules remain in table indefinitely.
//!    This preserves history but increases storage overhead (requires manual cleanup).
//!
//! 2. **Best-effort vs transactional**: Race condition allows room to execute despite
//!    cancellation request. Acceptable because cancellation is typically for
//!    optimization (reducing load) not correctness (preventing dangerous operation).

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for marking room schedules as cancelled (prevents execution).
///
/// WHY: Enables manual intervention to prevent scheduled rooms from executing
/// (e.g., cancel idle-triggered room if user became active again).
pub struct RoomCancel;

impl RoomCancel {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomCancel {
    fn name(&self) -> &'static str {
        "room:cancel"
    }

    /// Mark room schedule as cancelled.
    ///
    /// WHY this exists: Enables manual intervention to prevent scheduled rooms from
    /// executing. Useful for policy enforcement (cancel low-priority rooms under
    /// load) or debugging (cancel problematic room type during investigation).
    ///
    /// ARGUMENTS:
    /// - `id` or `schedule_id`: UUID of schedule to cancel
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{id, cancelled}` on success
    /// - `cancelled: true` if schedule was found and marked as cancelled
    /// - `cancelled: false` if schedule not found or already in terminal state
    /// - `E_INVALID_ARGS` if id missing
    /// - `E_INTERNAL` if kernel store not attached or update fails
    ///
    /// CONSTRAINTS:
    /// - Only "pending" schedules can be cancelled (enforced by Store layer)
    /// - Running/done schedules return `cancelled: false`
    ///
    /// BEHAVIOR:
    /// - Idempotent: cancelling already-cancelled schedule returns cancelled=true
    /// - Soft delete: schedule remains in table with status "cancelled"
    /// - RoomCoordinator skips cancelled schedules during tick polling
    ///
    /// USAGE:
    /// ```json
    /// {"id": "550e8400-e29b-41d4-a716-446655440000"}
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

        // WHY Store::cancel_room_schedule: Updates status to "cancelled" in
        // `room_schedules` table. Only affects schedules with status "pending"
        // (returns cancelled=false for running/done schedules).
        //
        // RACE CONDITION: If RoomCoordinator starts executing schedule between
        // this call and status update, room may run despite cancellation. Acceptable
        // because cancellation is best-effort (optimization) not transactional
        // (correctness requirement).
        let cancelled = store
            .cancel_room_schedule(id)
            .map_err(|e| KernelError::internal(format!("failed to cancel: {}", e)))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"id": id, "cancelled": cancelled}),
            ))
            .await;
        Ok(())
    }
}
