// MindService coordinates the Conclave - where MindManager, HeadManager, HandManager deliberate.
//
// On each tick interval, the service convenes the conclave. The three minds
// discuss recent activity and reach consensus on needs, wants, and LTM updates.
// This replaces the single-mind approach with a deliberative council.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::bus::{MessageData, MessageOp, Origin, Scope, respond};
use crate::history::Store;

use super::conclave::Conclave;
use super::mind_bundle::WakeMode;
use super::room::RoomDecision;
use super::{MindConfig, RuntimeBus};

pub struct MindService {
    bus: RuntimeBus,
    store: Arc<Store>,
    scopes: Vec<Scope>,
    mind_cfg: MindConfig,
    workspace: PathBuf,
}

impl MindService {
    pub fn new(
        bus: RuntimeBus,
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
            bus,
            store,
            scopes,
            mind_cfg,
            workspace,
        }
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(&self) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        let mut tick_counter: u64 = 0;
        tracing::debug!("mind service started");

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = ?e, "mind recv error");
                    continue;
                }
            };

            // Handle convene_conclave event from heads
            if msg.op == MessageOp::Event {
                if let MessageData::Event { kind, payload } = &msg.data {
                    if kind == "convene_conclave" {
                        let reason = payload.get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("requested by head");
                        tracing::info!(reason = %reason, "conclave requested by head");
                        tick_counter += 1;
                        self.convene_conclave(tick_counter, WakeMode::Normal).await;
                    }
                }
                continue;
            }

            // Only trigger on ping ticks
            if msg.op != MessageOp::Ping {
                continue;
            }

            let MessageData::Ping { tick, .. } = &msg.data else {
                continue;
            };

            tick_counter = *tick;

            // First tick always triggers boot sequence
            let is_boot_tick = *tick == 1;

            // Check if this tick triggers deliberation
            if !is_boot_tick {
                if self.mind_cfg.tick_interval == 0 {
                    continue;
                }
                if tick % self.mind_cfg.tick_interval != 0 {
                    continue;
                }
            }

            // Determine wake mode for boot tick
            let wake_mode = if is_boot_tick {
                self.determine_wake_mode()
            } else {
                WakeMode::Normal
            };

            tracing::info!(tick = tick, wake_mode = ?wake_mode, "conclave convening");
            self.convene_conclave(*tick, wake_mode).await;
        }
    }

    async fn convene_conclave(&self, tick: u64, wake_mode: WakeMode) {
        let conclave = Conclave::new(
            self.bus.clone(),
            self.store.clone(),
            self.scopes.clone(),
            self.workspace.clone(),
        );

        let room_id = format!("conclave:{}", tick);
        let wake_mode_str = format!("{:?}", wake_mode);

        let started_at_ms = now_ms();
        let started = Instant::now();
        self.bus
            .publish(
                respond::event(
                    "mind_service",
                    Scope::main(),
                    "conclave_call",
                    json!({
                        "id": room_id.clone(),
                        "tick": tick,
                        "wake_mode": wake_mode_str.clone(),
                        "started_at_ms": started_at_ms
                    }),
                )
                .with_origin(Origin::System),
            )
            .await;

        match conclave.convene(&room_id, wake_mode).await {
            Some(decision) => {
                tracing::info!(
                    room_id = %room_id,
                    needs = decision.needs.len(),
                    wants = decision.wants.len(),
                    "conclave concluded"
                );
            }
            None => {
                tracing::debug!(room_id = %room_id, "conclave made no decisions");
            }
        };

        let (status, decision_counts) = match self.store.get_conclave(&room_id) {
            Ok(Some(record)) => {
                let parsed: Option<RoomDecision> = serde_json::from_str(&record.decision).ok();
                let counts = parsed
                    .as_ref()
                    .map(|d| {
                        json!({
                            "needs": d.needs.len(),
                            "wants": d.wants.len(),
                            "ltm_ops": d.ltm_ops.len(),
                            "self_ops": d.self_ops.len()
                        })
                    })
                    .unwrap_or_else(|| json!({"needs": 0, "wants": 0, "ltm_ops": 0, "self_ops": 0}));
                (record.status, counts)
            }
            _ => (
                "unknown".to_string(),
                json!({"needs": 0, "wants": 0, "ltm_ops": 0, "self_ops": 0}),
            ),
        };

        self.bus
            .publish(
                respond::event(
                    "mind_service",
                    Scope::main(),
                    "conclave_done",
                    json!({
                        "id": room_id,
                        "tick": tick,
                        "wake_mode": wake_mode_str,
                        "status": status,
                        "duration_ms": started.elapsed().as_millis(),
                        "decision": decision_counts
                    }),
                )
                .with_origin(Origin::System),
            )
            .await;
    }

    fn determine_wake_mode(&self) -> WakeMode {
        // Check if we have any prior messages in the main scope
        let has_history = self.scopes.iter().any(|scope| {
            let scope_str = scope.to_string();
            self.store
                .recent(&scope_str, 1)
                .map(|msgs| !msgs.is_empty())
                .unwrap_or(false)
        });

        if has_history {
            WakeMode::Boot
        } else {
            WakeMode::Init
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
