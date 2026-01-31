// MindService coordinates the Conclave - where MindManager, HeadManager, HandManager deliberate.
//
// On each tick interval, the service convenes the conclave. The three minds
// discuss recent activity and reach consensus on needs, wants, and LTM updates.
// This replaces the single-mind approach with a deliberative council.

use std::sync::Arc;

use crate::bus::{MessageData, MessageOp, Scope};
use crate::history::Store;

use super::conclave::Conclave;
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

        tracing::info!(
            tick_interval = mind_cfg.tick_interval,
            "mind service configured (conclave mode)"
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
        tracing::info!("mind service started (conclave)");

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = ?e, "mind recv error");
                    continue;
                }
            };

            // Only trigger on ping ticks
            if msg.op != MessageOp::Ping {
                continue;
            }

            let MessageData::Ping { tick, .. } = &msg.data else {
                continue;
            };

            // Check if this tick triggers deliberation
            if self.mind_cfg.tick_interval == 0 {
                continue;
            }

            if tick % self.mind_cfg.tick_interval != 0 {
                continue;
            }

            tracing::info!(tick = tick, "conclave convening");
            self.convene_conclave(*tick).await;
        }
    }

    async fn convene_conclave(&self, tick: u64) {
        let conclave = Conclave::new(
            self.bus.clone(),
            self.store.clone(),
            self.scopes.clone(),
        );

        let room_id = format!("conclave:{}", tick);

        match conclave.convene(&room_id).await {
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
}
