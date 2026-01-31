// StatService publishes system stats to the bus for UI consumption.
//
// Observes the bus to maintain counters for active needs, goals, hands, and heads.
// Publishes Status messages every 5 seconds while the system is active.
// Pauses publishing and resets counters when the system goes idle.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::bus::{MessageData, MessageOp, NeedMsg, Origin, Scope, Stats, TaskMsg, respond};
use crate::history::Store;

use super::RuntimeBus;

const PUBLISH_INTERVAL_SECS: u64 = 5;

pub struct StatService {
    bus: RuntimeBus,
    store: Arc<Store>,
    state: Arc<RwLock<StatState>>,
}

struct StatState {
    idle: bool,
    tick: u64,
    // Transient counters (reset on idle)
    needs_count: u32,
    goals_count: u32,
    hands_running: u32,
    heads_busy: u32,
    // Cached store values (refreshed on idle)
    wants_count: u32,
    self_bytes: u32,
    ltm_bytes: u32,
    conclaves_count: u32,
}

impl Default for StatState {
    fn default() -> Self {
        Self {
            idle: true, // Start idle until activity
            tick: 0,
            needs_count: 0,
            goals_count: 0,
            hands_running: 0,
            heads_busy: 0,
            wants_count: 0,
            self_bytes: 0,
            ltm_bytes: 0,
            conclaves_count: 0,
        }
    }
}

impl StatService {
    pub fn new(bus: RuntimeBus, store: Arc<Store>) -> Self {
        Self {
            bus,
            store,
            state: Arc::new(RwLock::new(StatState::default())),
        }
    }

    pub fn start(self: Arc<Self>) {
        let svc = self.clone();
        tokio::spawn(async move {
            svc.run_message_loop().await;
        });

        let svc = self.clone();
        tokio::spawn(async move {
            svc.run_publish_loop().await;
        });

        tracing::debug!("stat service started");
    }

    async fn run_message_loop(&self) {
        let mut rx = self.bus.hub().read().await.subscribe_all();

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            let mut state = self.state.write().await;

            match (&msg.op, &msg.data, &msg.origin) {
                // Track tick from ping
                (MessageOp::Ping, MessageData::Ping { tick, .. }, _) => {
                    state.tick = *tick;
                }

                // Idle: reset counters and pause
                (MessageOp::Idle, _, _) => {
                    tracing::debug!("stat service going idle");
                    state.idle = true;
                    state.needs_count = 0;
                    state.goals_count = 0;
                    state.hands_running = 0;
                    state.heads_busy = 0;
                    // Cache store values on idle
                    self.refresh_store_cache(&mut state);
                }

                // Activity-starting messages: unpause
                (MessageOp::Need, MessageData::Need(NeedMsg::Request { .. }), _) => {
                    if state.idle {
                        tracing::debug!("stat service waking on need request");
                    }
                    state.idle = false;
                    state.needs_count = state.needs_count.saturating_add(1);
                }

                (MessageOp::Chat, _, Origin::Human) => {
                    if state.idle {
                        tracing::debug!("stat service waking on human chat");
                    }
                    state.idle = false;
                }

                (MessageOp::Task, MessageData::Task(TaskMsg::Request { .. }), _) => {
                    if state.idle {
                        tracing::debug!("stat service waking on task request");
                    }
                    state.idle = false;
                    state.goals_count = state.goals_count.saturating_add(1);
                }

                // Track need lifecycle
                (MessageOp::Need, MessageData::Need(NeedMsg::Fulfilled { .. }), _)
                | (MessageOp::Need, MessageData::Need(NeedMsg::Expired { .. }), _) => {
                    state.needs_count = state.needs_count.saturating_sub(1);
                }

                // Track task/goal lifecycle
                (MessageOp::Task, MessageData::Task(TaskMsg::Assigned { .. }), _) => {
                    state.hands_running = state.hands_running.saturating_add(1);
                }

                (MessageOp::Task, MessageData::Task(TaskMsg::Result { .. }), _) => {
                    state.goals_count = state.goals_count.saturating_sub(1);
                    state.hands_running = state.hands_running.saturating_sub(1);
                }

                // Track head lifecycle
                (MessageOp::Wake, _, _) => {
                    state.heads_busy = state.heads_busy.saturating_add(1);
                }

                (MessageOp::Sleep, _, _) => {
                    state.heads_busy = state.heads_busy.saturating_sub(1);
                }

                _ => {}
            }
        }
    }

    async fn run_publish_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(PUBLISH_INTERVAL_SECS));

        loop {
            interval.tick().await;

            let state = self.state.read().await;
            if state.idle {
                continue;
            }

            let stats = Stats {
                tick: state.tick,
                needs_count: state.needs_count,
                goals_count: state.goals_count,
                wants_count: state.wants_count,
                hands_running: state.hands_running,
                hands_total: 0, // TODO: track from config
                heads_busy: state.heads_busy,
                heads_total: 0, // TODO: track from config
                next_conclave_secs: 0, // TODO: track from mind service
                self_bytes: state.self_bytes,
                ltm_bytes: state.ltm_bytes,
                conclaves_count: state.conclaves_count,
            };

            drop(state); // Release lock before publishing

            self.bus
                .publish(respond::status("_stat_service", Scope::main(), stats).with_origin(Origin::System))
                .await;
        }
    }

    fn refresh_store_cache(&self, state: &mut StatState) {
        state.wants_count = self.store.count_wants().unwrap_or(0) as u32;
        state.conclaves_count = self
            .store
            .list_conclaves(1000)
            .map(|c| c.len())
            .unwrap_or(0) as u32;
        state.self_bytes = self
            .store
            .get_conclave_self()
            .map(|s| s.len())
            .unwrap_or(0) as u32;
        state.ltm_bytes = self
            .store
            .get_head_ltm("conclave")
            .map(|s| s.len())
            .unwrap_or(0) as u32;
    }
}
