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
use super::mind_bundle::{WakeMode, FeverMode};
use super::room::RoomDecision;
use super::{MindConfig, RuntimeBus};

pub struct MindService {
    bus: RuntimeBus,
    store: Arc<Store>,
    scopes: Vec<Scope>,
    workspace: PathBuf,
    fever: FeverMode,
    conclave_on_boot: bool,
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
        let mut rx = self.bus.hub().read().await.subscribe_all();
        let mut conclave_seq: u64 = 0;
        tracing::debug!("mind service started");

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = ?e, "mind recv error");
                    continue;
                }
            };

            // Meth mode: convene autonomy immediately on any idle (never conclave)
            if self.fever == FeverMode::Meth && msg.op == MessageOp::Idle {
                tracing::info!("meth mode: immediate idle; convening autonomy");
                conclave_seq += 1;
                self.convene_autonomy(conclave_seq).await;
                continue;
            }

            // Handle events (requests and idle milestones)
            if msg.op == MessageOp::Event {
                if let MessageData::Event { kind, payload } = &msg.data {
                    if kind == "convene_conclave" {
                        let reason = payload.get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("requested by head");
                        tracing::info!(reason = %reason, "conclave requested by head");
                        conclave_seq += 1;
                        self.convene_conclave(conclave_seq, WakeMode::Normal).await;
                    } else if kind == "slow_idle" {
                        tracing::info!("slow idle reached; convening autonomy");
                        conclave_seq += 1;
                        self.convene_autonomy(conclave_seq).await;
                    } else if kind == "deep_idle" {
                        tracing::info!("deep idle reached; convening conclave");
                        conclave_seq += 1;
                        self.convene_conclave(conclave_seq, WakeMode::Normal).await;
                    } else if kind == "collective_reboot" {
                        let reason = payload
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("reboot");
                        let epoch = payload
                            .get("epoch")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        tracing::warn!(epoch, reason = %reason, "collective reboot observed; convening conclave");
                        conclave_seq += 1;
                        self.convene_conclave(conclave_seq, WakeMode::Init).await;
                    }
                }
                continue;
            }

            // Only trigger on ping ticks (boot only)
            if msg.op != MessageOp::Ping {
                continue;
            }

            let MessageData::Ping { tick, .. } = &msg.data else {
                continue;
            };

            // First tick triggers boot conclave if --conclave flag was passed
            let is_boot_tick = *tick == 1;

            if !is_boot_tick {
                continue;
            }

            if !self.conclave_on_boot {
                tracing::debug!(tick = tick, "boot tick: --conclave not set, skipping");
                continue;
            }

            // Determine wake mode for boot tick (init vs regular boot)
            let wake_mode = self.determine_wake_mode();

            conclave_seq += 1;
            tracing::info!(tick = tick, wake_mode = ?wake_mode, "conclave convening (--conclave)");
            self.convene_conclave(conclave_seq, wake_mode).await;
        }
    }

    async fn convene_conclave(&self, tick: u64, wake_mode: WakeMode) {
        let conclave = Conclave::new(
            self.bus.clone(),
            self.store.clone(),
            self.scopes.clone(),
            self.workspace.clone(),
        ).with_fever(self.fever.clone());

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

    async fn convene_autonomy(&self, seq: u64) {
        let conclave = Conclave::new(
            self.bus.clone(),
            self.store.clone(),
            self.scopes.clone(),
            self.workspace.clone(),
        ).with_fever(self.fever.clone());

        let room_id = format!("autonomy:{}", seq);
        let started_at_ms = now_ms();
        let started = Instant::now();

        self.bus
            .publish(
                respond::event(
                    "mind_service",
                    Scope::main(),
                    "autonomy_call",
                    json!({
                        "id": room_id.clone(),
                        "seq": seq,
                        "started_at_ms": started_at_ms
                    }),
                )
                .with_origin(Origin::System),
            )
            .await;

        match conclave.autonomy(&room_id, WakeMode::Normal).await {
            Some(decision) => {
                tracing::info!(
                    room_id = %room_id,
                    needs = decision.needs.len(),
                    wants = decision.wants.len(),
                    "autonomy concluded"
                );
            }
            None => {
                tracing::debug!(room_id = %room_id, "autonomy made no decisions");
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
                    "autonomy_done",
                    json!({
                        "id": room_id,
                        "seq": seq,
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
