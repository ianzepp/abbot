// MindService coordinates the Conclave - where MindManager, HeadManager, HandManager deliberate.
//
// On each tick interval, the service convenes the conclave. The three minds
// discuss recent activity and reach consensus on needs, wants, and LTM updates.
// This replaces the single-mind approach with a deliberative council.

use std::sync::Arc;

use crate::bus::{MessageData, MessageOp, Scope};
use crate::history::Store;

use super::conclave::Conclave;
use super::mind_bundle::WakeMode;
use super::{MindConfig, RuntimeBus};

pub struct MindService {
    bus: RuntimeBus,
    store: Arc<Store>,
    scopes: Vec<Scope>,
    mind_cfg: MindConfig,
}

impl MindService {
    pub fn new(
        bus: RuntimeBus,
        store: Arc<Store>,
        _head_id: impl Into<String>,
        scopes: Vec<Scope>,
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
        );

        let room_id = format!("conclave:{}", tick);

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
        }
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
