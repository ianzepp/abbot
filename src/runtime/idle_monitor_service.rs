use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::bus::{Message, MessageOp, Origin, Scope, respond};

use super::RuntimeBus;
use super::app_config::HarnessToml;
use super::mind_bundle::FeverMode;

pub struct IdleMonitorService {
    bus: RuntimeBus,
    fever: FeverMode,
    workspace_path: PathBuf,
}

impl IdleMonitorService {
    pub fn new(bus: RuntimeBus, workspace_path: PathBuf) -> Self {
        Self {
            bus,
            fever: FeverMode::None,
            workspace_path,
        }
    }

    pub fn with_fever(mut self, fever: FeverMode) -> Self {
        self.fever = fever;
        self
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(self: Arc<Self>) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        tracing::debug!("idle monitor service started");

        let harness = HarnessToml::from_workspace(&self.workspace_path);
        tracing::debug!(
            slow_idle_ms = harness.slow_idle_ms(),
            deep_idle_ms = harness.deep_idle_ms(),
            "loaded harness config"
        );

        let mut state = IdleState::new(harness.slow_idle_ms(), harness.deep_idle_ms());

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            let now = now_ms();
            state.observe(&msg, now);
            if let Some(event) = state.maybe_emit(now) {
                self.bus
                    .publish(
                        respond::event("idle_monitor", Scope::main(), event.kind, event.payload)
                            .with_origin(Origin::System),
                    )
                    .await;
            }
        }
    }
}

struct IdleEvent {
    kind: &'static str,
    payload: serde_json::Value,
}

struct IdleState {
    idle_since_ms: Option<i64>,
    last_activity_ms: i64,
    slow_idle_emitted: bool,
    deep_idle_emitted: bool,
    seen_activity: bool,
    slow_idle_threshold_ms: i64,
    deep_idle_threshold_ms: i64,
}

impl IdleState {
    fn new(slow_idle_threshold_ms: i64, deep_idle_threshold_ms: i64) -> Self {
        Self {
            idle_since_ms: None,
            last_activity_ms: now_ms(),
            slow_idle_emitted: false,
            deep_idle_emitted: false,
            seen_activity: false,
            slow_idle_threshold_ms,
            deep_idle_threshold_ms,
        }
    }

    fn observe(&mut self, msg: &Message, now_ms: i64) {
        if msg.op == MessageOp::Idle && msg.origin == Origin::System {
            self.idle_since_ms.get_or_insert(now_ms);
            return;
        }

        if is_activity(msg) {
            self.seen_activity = true;
            self.last_activity_ms = now_ms;
            self.idle_since_ms = None;
            self.slow_idle_emitted = false;
            self.deep_idle_emitted = false;
        }
    }

    fn maybe_emit(&mut self, now_ms: i64) -> Option<IdleEvent> {
        let Some(idle_since_ms) = self.idle_since_ms else {
            return None;
        };

        // Only emit if we've seen real activity at least once, to avoid firing
        // just because the daemon started and nothing happened.
        if !self.seen_activity {
            return None;
        }

        let idle_for_ms = now_ms.saturating_sub(idle_since_ms);

        if !self.slow_idle_emitted && idle_for_ms >= self.slow_idle_threshold_ms {
            self.slow_idle_emitted = true;
            return Some(IdleEvent {
                kind: "slow_idle",
                payload: json!({
                    "idle_since_ms": idle_since_ms,
                    "idle_for_ms": idle_for_ms,
                    "last_activity_ms": self.last_activity_ms
                }),
            });
        }

        if !self.deep_idle_emitted && idle_for_ms >= self.deep_idle_threshold_ms {
            self.deep_idle_emitted = true;
            return Some(IdleEvent {
                kind: "deep_idle",
                payload: json!({
                    "idle_since_ms": idle_since_ms,
                    "idle_for_ms": idle_for_ms,
                    "last_activity_ms": self.last_activity_ms
                }),
            });
        }

        None
    }
}

fn is_activity(msg: &Message) -> bool {
    match msg.op {
        MessageOp::Ping => false,
        MessageOp::Status => false,
        MessageOp::Idle => false,
        MessageOp::Event => false,

        // Consider everything else as activity, including system chat.
        _ => true,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
