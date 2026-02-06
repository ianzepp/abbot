//! Room Coordinator - Idle-driven room scheduling and dispatch
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The coordinator is a long-running service that monitors kernel activity and
//! idle state, dispatching room sessions (conclave or autonomy) when appropriate.
//! It replaces MindService with the same tick-subscription + idle-detection
//! pattern but dispatches via `room:create` + `room:run` syscalls instead of
//! the deprecated `mind:conclave`/`mind:autonomy` frames.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Tick-driven polling: Subscribes to `tick:subscribe` and evaluates idle
//!   state on each tick rather than using timers. This ensures consistent
//!   behavior regardless of system clock drift.
//! - Idle detection hierarchy: slow idle (minutes) triggers autonomy, deep
//!   idle (longer) triggers full conclave. Meth mode overrides both.
//! - Epoch-based reboot: When the reboot epoch changes (triggered by a
//!   control:reboot_collective decision), runs an init conclave to reload.
//!
//! CONCURRENCY
//! ===========
//! Uses `Arc<Self>` + `tokio::spawn` pattern. The coordinator holds no mutable
//! state behind the Arc; all state is local to the `run()` loop.
//!
//! TRADE-OFFS
//! ==========
//! - Dispatches rooms via syscalls (room:create + room:stream + room:run)
//!   rather than calling RoomRunner directly. This adds overhead but ensures
//!   room lifecycle events are properly emitted and streams are managed.

use std::path::PathBuf;
use std::sync::Arc;

use crate::Scope;
use crate::history::Store;
use crate::runtime::{Kernel, reboot_epoch};

use super::config::RoomConfig;
use super::bundle::{FeverMode, WakeMode};

// =============================================================================
// COORDINATOR
// =============================================================================

/// Schedules and dispatches room sessions based on kernel idle state.
///
/// WHY this exists: Without active scheduling, rooms would only run when
/// explicitly requested. The coordinator enables autonomous operation by
/// detecting idle periods and triggering reflection (autonomy) or strategic
/// planning (conclave) sessions.
pub struct RoomCoordinator {
    _store: Arc<Store>,
    _scopes: Vec<Scope>,
    workspace: PathBuf,
    fever: FeverMode,
    conclave_on_boot: bool,
}

impl RoomCoordinator {
    pub fn new(
        store: Arc<Store>,
        _head_id: impl Into<String>,
        scopes: Vec<Scope>,
        workspace: PathBuf,
    ) -> Self {
        let room_cfg = RoomConfig::from_config();

        tracing::debug!(
            tick_interval = room_cfg.tick_interval,
            "room coordinator configured"
        );

        Self {
            _store: store,
            _scopes: scopes,
            workspace,
            fever: room_cfg.fever,
            conclave_on_boot: false,
        }
    }

    pub fn with_fever(mut self, fever: FeverMode) -> Self {
        self.fever = fever;
        self
    }

    pub fn with_conclave_on_boot(mut self, enabled: bool) -> Self {
        self.conclave_on_boot = enabled;
        self
    }

    /// Spawn the coordinator as a background task.
    ///
    /// WHY Arc<Self>: The coordinator is shared across the spawn boundary.
    /// No interior mutability needed because all state is local to run().
    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    // =========================================================================
    // MAIN LOOP
    // =========================================================================
    //
    // WHY tick-based: Using tick:subscribe ensures the coordinator runs at
    // predictable intervals aligned with the kernel's global tick, rather
    // than drifting with tokio::time::interval.

    async fn run(&self) {
        let room_cfg = RoomConfig::from_config();
        let harness = crate::runtime::app_config::HarnessToml::from_workspace(&self.workspace);

        tracing::debug!(
            tick_interval_s = room_cfg.tick_interval,
            slow_idle_ms = harness.slow_idle_ms(),
            deep_idle_ms = harness.deep_idle_ms(),
            "room coordinator started"
        );

        let Some(k) = Kernel::get() else {
            return;
        };
        let dispatcher = k.dispatcher().await;
        let req = crate::kernel::Frame::req("tick:subscribe", serde_json::json!({}))
            .with_actor("system/room_coordinator");
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut tick_rx = dispatcher.dispatch(req, k.workspace().to_path_buf(), cancel);

        let mut last_run_ms: i64 = 0;
        let run_every_ms: i64 = (room_cfg.tick_interval as i64).saturating_mul(1000);
        let mut seq: u64 = 0;
        let mut boot_done = false;
        let mut last_epoch = reboot_epoch();
        let mut last_activity_seq: u64 = 0;
        let mut slow_emitted = false;
        let mut deep_emitted = false;
        let mut meth_last_activity_seq: u64 = 0;

        loop {
            let Some(frame) = tick_rx.recv().await else {
                return;
            };
            if frame.op != crate::kernel::FrameOp::Event {
                continue;
            }
            let Some(data) = frame.data else {
                continue;
            };
            if data.get("kind").and_then(|v| v.as_str()) != Some("SIGTICK") {
                continue;
            }

            // WHY rate limit: Prevents running rooms more often than tick_interval,
            // even if ticks arrive faster than expected.
            let now_ms = data
                .get("now_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or_else(now_ms);
            if run_every_ms > 0 {
                if last_run_ms != 0 {
                    let dt = now_ms.saturating_sub(last_run_ms);
                    if dt < run_every_ms {
                        continue;
                    }
                }
                last_run_ms = now_ms;
            }

            // WHY boot conclave: On first start, run a conclave to establish
            // initial state and pick up any pending work.
            if self.conclave_on_boot && !boot_done {
                boot_done = true;
                let wake_mode = self.determine_wake_mode();
                seq += 1;
                let _ = self.dispatch_room(&k, true, seq, wake_mode).await;
                continue;
            }

            // WHY epoch check: A reboot_collective control proposal bumps the
            // epoch. When detected, run an init conclave to reload config/plugins.
            let epoch = reboot_epoch();
            if epoch != last_epoch {
                last_epoch = epoch;
                seq += 1;
                let _ = self
                    .dispatch_room(&k, true, seq, WakeMode::Init)
                    .await;
                continue;
            }

            // WHY activity tracking: Reset idle timers when new activity is
            // detected, ensuring we only trigger idle rooms after genuine idle.
            let activity_seq = k.activity_seq();
            if activity_seq != last_activity_seq {
                last_activity_seq = activity_seq;
                slow_emitted = false;
                deep_emitted = false;
            }

            // WHY skip if no activity: Don't fire idle rooms before the system
            // has done any real work. Prevents false autonomy on fresh boot.
            if activity_seq == 0 {
                continue;
            }

            let idle = if let Some(ems) = k.ems() {
                let ems = ems.lock().unwrap();
                let need_active = ems.select("needs", Some(&serde_json::json!({"status": {"$in": ["pending", "running"]}})), None, None, Some(1), None).map(|r| r.len()).unwrap_or(0);
                let task_active = ems.select("tasks", Some(&serde_json::json!({"status": {"$in": ["pending", "running"]}})), None, None, Some(1), None).map(|r| r.len()).unwrap_or(0);
                need_active == 0 && task_active == 0
            } else { true };
            if !idle {
                continue;
            }

            let idle_for_ms = now_ms.saturating_sub(k.activity_last_ms());

            // WHY meth mode: In high-fever mode, run autonomy immediately on
            // first idle after each activity period. Skips slow/deep thresholds.
            if self.fever == FeverMode::Meth {
                if meth_last_activity_seq != activity_seq {
                    meth_last_activity_seq = activity_seq;
                    seq += 1;
                    let _ = self
                        .dispatch_room(&k, false, seq, WakeMode::Normal)
                        .await;
                }
                continue;
            }

            // WHY deep before slow: Deep idle triggers a full conclave (strategic
            // review), while slow idle triggers lighter autonomy (tactical check).
            // Check deep first to avoid triggering both on the same idle period.
            if !deep_emitted && idle_for_ms >= harness.deep_idle_ms() {
                deep_emitted = true;
                seq += 1;
                let _ = self
                    .dispatch_room(&k, true, seq, WakeMode::Normal)
                    .await;
                continue;
            }

            if !slow_emitted && idle_for_ms >= harness.slow_idle_ms() {
                slow_emitted = true;
                seq += 1;
                let _ = self
                    .dispatch_room(&k, false, seq, WakeMode::Normal)
                    .await;
                continue;
            }
        }
    }

    // =========================================================================
    // ROOM DISPATCH
    // =========================================================================
    //
    // WHY three-step dispatch (create + stream + run): The room:run syscall
    // requires a stream to be open first (stream-then-run protocol). This
    // ensures room events are captured even if the coordinator doesn't consume
    // them, preventing dropped events.

    async fn dispatch_room(
        &self,
        k: &Kernel,
        conclave: bool,
        seq: u64,
        wake_mode: WakeMode,
    ) -> Result<(), ()> {
        let dispatcher = k.dispatcher().await;
        let wake_mode_str = match wake_mode {
            WakeMode::Init => "init",
            _ => "normal",
        };

        let room_type = if conclave { "conclave" } else { "autonomy" };

        // -------------------------------------------------------------------------
        // PHASE 1: CREATE ROOM
        // Allocates a room_id in the kernel's room registry.
        // -------------------------------------------------------------------------
        let create_req = crate::kernel::Frame::req(
            "room:create",
            serde_json::json!({
                "type": room_type,
                "scope": "main",
            }),
        )
        .with_actor("system/room_coordinator");

        let mut rx = dispatcher.dispatch(
            create_req,
            k.workspace().to_path_buf(),
            tokio_util::sync::CancellationToken::new(),
        );

        let room_id = loop {
            let Some(frame) = rx.recv().await else {
                tracing::error!("room:create stream ended without response");
                return Err(());
            };
            if frame.op == crate::kernel::FrameOp::Ok {
                if let Some(data) = frame.data {
                    if let Some(id) = data.get("room_id").and_then(|v| v.as_str()) {
                        break id.to_string();
                    }
                }
                tracing::error!("room:create ok but no room_id");
                return Err(());
            }
            if frame.op == crate::kernel::FrameOp::Error {
                tracing::error!("room:create failed");
                return Err(());
            }
            if frame.op == crate::kernel::FrameOp::Done {
                tracing::error!("room:create done without ok");
                return Err(());
            }
        };

        // -------------------------------------------------------------------------
        // PHASE 2: OPEN STREAM
        // WHY before run: The stream-then-run protocol ensures no events are
        // dropped between room creation and deliberation start.
        // -------------------------------------------------------------------------
        let stream_req = crate::kernel::Frame::req(
            "room:stream",
            serde_json::json!({"room_id": room_id}),
        )
        .with_actor("system/room_coordinator");

        let cancel = tokio_util::sync::CancellationToken::new();
        let mut stream_rx = dispatcher.dispatch(
            stream_req,
            k.workspace().to_path_buf(),
            cancel.clone(),
        );

        // -------------------------------------------------------------------------
        // PHASE 3: EXECUTE DELIBERATION
        // Blocks until the room completes (consensus or timeout).
        // -------------------------------------------------------------------------
        let run_req = crate::kernel::Frame::req(
            "room:run",
            serde_json::json!({
                "room_id": room_id,
                "wake_mode": wake_mode_str,
                "context": format!("scheduled seq={}", seq),
            }),
        )
        .with_actor("system/room_coordinator");

        let mut run_rx = dispatcher.dispatch(
            run_req,
            k.workspace().to_path_buf(),
            tokio_util::sync::CancellationToken::new(),
        );

        while let Some(frame) = run_rx.recv().await {
            if matches!(frame.op, crate::kernel::FrameOp::Ok | crate::kernel::FrameOp::Error | crate::kernel::FrameOp::Done) {
                break;
            }
        }

        // WHY cancel stream: The stream subscription outlives the room:run call.
        // Cancelling it releases the stream channel and unblocks the stream syscall.
        cancel.cancel();
        let _ = stream_rx.recv().await;

        Ok(())
    }

    fn determine_wake_mode(&self) -> WakeMode {
        WakeMode::Normal
    }
}

// =============================================================================
// HELPERS
// =============================================================================

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
