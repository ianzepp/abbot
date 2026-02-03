// MindService schedules conclave/autonomy rooms.
//
// MindService polls kernel activity + idle state and invokes mind:* syscalls.

use std::path::PathBuf;
use std::sync::Arc;
// Duration reserved for future tick-based sleeps/timeouts.

use crate::Scope;
use crate::history::Store;
use crate::runtime::{Kernel, reboot_epoch};

use super::app_config::HarnessToml;
use super::mind_bundle::{FeverMode, WakeMode};
use super::MindConfig;

pub struct MindService {
    _store: Arc<Store>,
    _scopes: Vec<Scope>,
    workspace: PathBuf,
    fever: FeverMode,
    conclave_on_boot: bool,
}

impl MindService {
    pub fn new(
        store: Arc<Store>,
        _head_id: impl Into<String>,
        scopes: Vec<Scope>,
        workspace: PathBuf,
    ) -> Self {
        let mind_cfg = MindConfig::from_env();

        tracing::debug!(
            tick_interval = mind_cfg.tick_interval,
            "mind service configured"
        );

        Self {
            _store: store,
            _scopes: scopes,
            workspace,
            fever: FeverMode::None,
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

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(&self) {
        let mind_cfg = MindConfig::from_env();
        let harness = HarnessToml::from_workspace(&self.workspace);

        tracing::debug!(
            tick_interval_s = mind_cfg.tick_interval,
            slow_idle_ms = harness.slow_idle_ms(),
            deep_idle_ms = harness.deep_idle_ms(),
            "mind service started"
        );

        let Some(k) = Kernel::get() else {
            return;
        };
        let dispatcher = k.dispatcher().await;
        let req = crate::kernel::Frame::req("tick:subscribe", serde_json::json!({}))
            .with_actor("system/mind_service");
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut tick_rx = dispatcher.dispatch(req, k.workspace().to_path_buf(), cancel);

        let mut last_run_ms: i64 = 0;
        let run_every_ms: i64 = (mind_cfg.tick_interval as i64).saturating_mul(1000);
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

            let now_ms = data.get("now_ms").and_then(|v| v.as_i64()).unwrap_or_else(now_ms);
            if run_every_ms > 0 {
                if last_run_ms != 0 {
                    let dt = now_ms.saturating_sub(last_run_ms);
                    if dt < run_every_ms {
                        continue;
                    }
                }
                last_run_ms = now_ms;
            }

            // Boot conclave (at most once per daemon start).
            if self.conclave_on_boot && !boot_done {
                boot_done = true;
                let wake_mode = self.determine_wake_mode();
                seq += 1;
                let _ = self.dispatch_mind_convene(&k, true, seq, wake_mode).await;
                continue;
            }

            // Reboot epoch changed -> run init conclave.
            let epoch = reboot_epoch();
            if epoch != last_epoch {
                last_epoch = epoch;
                seq += 1;
                let _ = self.dispatch_mind_convene(&k, true, seq, WakeMode::Init).await;
                continue;
            }

            let activity_seq = k.activity_seq();
            if activity_seq != last_activity_seq {
                last_activity_seq = activity_seq;
                slow_emitted = false;
                deep_emitted = false;
            }

            // Don't fire idle-based behaviors until we've seen real activity.
            if activity_seq == 0 {
                continue;
            }

            let (need_q, need_active) = k.needs().counts().await;
            let (task_q, task_running, _task_done) = k.tasks().counts().await;
            let idle = need_q == 0 && need_active == 0 && task_q == 0 && task_running == 0;
            if !idle {
                continue;
            }

            let idle_for_ms = now_ms.saturating_sub(k.activity_last_ms());

            // Meth mode: run autonomy once per activity period on first idle.
            if self.fever == FeverMode::Meth {
                if meth_last_activity_seq != activity_seq {
                    meth_last_activity_seq = activity_seq;
                    seq += 1;
                    let _ = self.dispatch_mind_convene(&k, false, seq, WakeMode::Normal).await;
                }
                continue;
            }

            if !deep_emitted && idle_for_ms >= harness.deep_idle_ms() {
                deep_emitted = true;
                seq += 1;
                let _ = self.dispatch_mind_convene(&k, true, seq, WakeMode::Normal).await;
                continue;
            }

            if !slow_emitted && idle_for_ms >= harness.slow_idle_ms() {
                slow_emitted = true;
                seq += 1;
                let _ = self.dispatch_mind_convene(&k, false, seq, WakeMode::Normal).await;
                continue;
            }
        }
    }

    async fn dispatch_mind_convene(
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

        let name = if conclave {
            "mind:convene_conclave"
        } else {
            "mind:convene_autonomy"
        };

        let req = crate::kernel::Frame::req(
            name,
            serde_json::json!({
                "scope": "main",
                "reason": "scheduled",
                "wake_mode": wake_mode_str,
                "seq": seq,
            }),
        )
        .with_actor("system/mind_service");

        let mut rx = dispatcher.dispatch(
            req,
            k.workspace().to_path_buf(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;
        Ok(())
    }

    fn determine_wake_mode(&self) -> WakeMode {
        WakeMode::Normal
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
