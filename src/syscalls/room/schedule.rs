//! Room:Schedule - Persist room schedule for future execution
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall persists room schedules to the `room_schedules` SQLite table, enabling
//! deferred execution at a specified time. Schedules track execution status (pending,
//! running, done, cancelled, failed), retry attempts, and last error for observability.
//!
//! **Schedule lifecycle:**
//! 1. Create schedule with `room:schedule` (status: pending)
//! 2. RoomCoordinator polls schedules on each tick
//! 3. When `run_after_ms` passes, coordinator executes via `room:create` + `room:run`
//! 4. Status transitions: pending → running → done (or failed if error occurs)
//! 5. Failed schedules can be retried (attempts counter increments, last_error stored)
//!
//! **Use cases:**
//! - RoomCoordinator scheduling: idle detection triggers autonomy/conclave schedules
//! - Deferred execution: schedule room to run at specific time (e.g., nightly reflection)
//! - Retry logic: failed rooms can be rescheduled with exponential backoff
//!
//! **Persistence rationale:**
//! - WHY persist: Schedules survive kernel restarts (important for long-running daemons)
//! - WHY SQLite: Enables querying schedules by status, room_type, or scope
//! - WHY status tracking: Enables observability (see which rooms ran, when, outcome)
//!
//! **Integration points:**
//! - `Store::insert_room_schedule()` - Persists schedule to `room_schedules` table
//! - `Store::list_room_schedules()` - Queries schedules with filtering
//! - RoomCoordinator tick loop polls for schedules with `run_after_ms` <= now
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Explicit scheduling**: Room:schedule is separate from room:run so callers can
//!   persist schedules without immediate execution (deferred execution pattern).
//! - **Flexible timing**: `run_after_ms` defaults to "now" but can specify future time
//!   (enables delayed execution, batching, rate limiting).
//! - **Observability via status**: Status transitions are persisted so operators can
//!   debug room execution failures (see last_error, attempts count).
//! - **Constraints for extensibility**: `constraints` field (JSON) enables future
//!   features like resource limits, participant overrides, or dependency chains.
//!
//! CONCURRENCY
//! ===========
//! - Safe for concurrent execution (SQLite handles write serialization)
//! - Multiple schedules can be created simultaneously without coordination
//! - Schedule IDs are UUIDs (collision probability negligible)
//!
//! SECURITY MODEL
//! ==============
//! - No permission checks (any actor can schedule rooms)
//! - WHY permissive: Rooms are internal coordination mechanisms. Scheduling doesn't
//!   grant execution control (RoomCoordinator decides when to execute based on policy).
//! - Room schedules are visible to all actors (no per-room access control)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Persist all schedules vs ephemeral**: All schedules are persisted to SQLite
//!    regardless of execution time. This increases storage overhead but enables
//!    historical analysis (which rooms ran, when, outcome).
//!
//! 2. **Immediate vs polling-based execution**: Schedules are checked on coordinator
//!    tick (not via timers). This adds latency (up to tick_interval) but ensures
//!    consistent behavior regardless of system clock drift.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, RoomKind, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for persisting room schedules to SQLite for future execution.
///
/// WHY: Enables deferred execution and retry logic. Schedules survive kernel
/// restarts and can be queried/modified before execution.
pub struct RoomSchedule;

impl RoomSchedule {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomSchedule {
    fn name(&self) -> &'static str {
        "room:schedule"
    }

    /// Persist room schedule for future execution.
    ///
    /// WHY this exists: Separates schedule creation from execution so rooms can
    /// be deferred to specific times (idle detection, nightly reflection) and
    /// retried on failure (exponential backoff, error recovery).
    ///
    /// ARGUMENTS:
    /// - `room_type` or `type`: Room type ("conclave", "autonomy", "work")
    /// - `scope`: Logical context (default: "main")
    /// - `run_after_ms`: Unix timestamp (ms) when room should execute (default: now)
    /// - `reason`: Why room was scheduled (e.g., "slow idle", "nightly reflection")
    /// - `wake_mode`: "normal" or "init" (default: "normal")
    /// - `constraints`: JSON object for future extensibility (default: {})
    /// - `context`: Optional string describing scheduling context
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{schedule_id, room_type, scope, run_after_ms}` on success
    /// - `E_INVALID_ARGS` if room_type is invalid
    /// - `E_INTERNAL` if kernel store not attached or insert fails
    ///
    /// USAGE:
    /// ```json
    /// {"room_type": "autonomy", "scope": "main", "reason": "slow idle", "run_after_ms": 1704067200000}
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

        // WHY accept both "room_type" and "type": Backward compatibility with existing
        // callers that may use either field name. "room_type" is preferred to avoid
        // collision with Rust keyword "type".
        let room_type = data
            .get("room_type")
            .or_else(|| data.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if RoomKind::from_str(room_type).is_none() {
            return Err(KernelError::invalid_args(
                "room_type must be 'conclave', 'autonomy', or 'work'",
            ));
        }

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();

        // WHY default to now: Most schedules should execute immediately (idle detection
        // triggers). Explicit run_after_ms enables deferred execution without separate
        // "schedule later" syscall.
        let run_after_ms = data
            .get("run_after_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64
            });

        let reason = data
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("scheduled")
            .trim();

        let wake_mode = data
            .get("wake_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("normal")
            .trim();

        // WHY constraints as JSON: Enables future features (resource limits, participant
        // overrides, dependency chains) without schema changes. Stored as string for
        // SQLite compatibility, parsed when schedule executes.
        let constraints_json = data
            .get("constraints")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "{}".to_string());

        let context = data
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let id = Uuid::new_v4().to_string();

        // WHY Store::insert_room_schedule: Persists to `room_schedules` table with
        // status=pending, attempts=0, last_error=null. RoomCoordinator polls this
        // table on each tick and executes schedules with run_after_ms <= now.
        store
            .insert_room_schedule(
                &id,
                room_type,
                scope,
                run_after_ms,
                reason,
                wake_mode,
                &constraints_json,
                context,
            )
            .map_err(|e| KernelError::internal(format!("failed to insert schedule: {}", e)))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"schedule_id": id, "room_type": room_type, "scope": scope, "run_after_ms": run_after_ms}),
            ))
            .await;
        Ok(())
    }
}
