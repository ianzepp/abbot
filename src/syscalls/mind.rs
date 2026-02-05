use std::sync::Arc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::{Conclave, ConclaveTrace, Kernel, WakeMode};
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

pub struct MindConclave;

impl MindConclave {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for MindConclave {
    fn name(&self) -> &'static str {
        "mind:conclave"
    }

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

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();

        let wake_mode = data
            .get("wake_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("normal")
            .trim();
        let wake_mode = match wake_mode {
            "init" => WakeMode::Init,
            _ => WakeMode::Normal,
        };

        let room_id = match data.get("room_id").and_then(|v| v.as_str()) {
            Some(s) => match Uuid::parse_str(s) {
                Ok(id) => id,
                Err(_) => {
                    tracing::warn!(provided = %s, "invalid room_id UUID, generating new one");
                    Uuid::new_v4()
                }
            },
            None => Uuid::new_v4(),
        };
        let room_id_str = room_id.to_string();

        let actor = ctx.actor.clone().unwrap_or_else(|| "system/mind".to_string());
        let trace = ConclaveTrace::new(ctx.call_id, tx.clone(), actor.clone());

        let _ = tx
            .send(
                Frame::event(
                    ctx.call_id,
                    json!({
                        "kind": "mind:start",
                        "room_id": room_id_str,
                        "type": "conclave",
                        "scope": scope,
                        "wake_mode": format!("{:?}", wake_mode),
                    }),
                )
                .with_actor(actor.clone())
                .with_name("mind:conclave"),
            )
            .await;

        let scopes = vec![crate::Scope::from(scope)];
        let conclave = Conclave::new(store.clone(), scopes, k.workspace().to_path_buf());
        let decision = conclave
            .convene_with_trace(&room_id_str, wake_mode, Some(trace))
            .await;

        let record = store.get_conclave(&room_id_str).ok().flatten();

        let _ = tx
            .send(
                Frame::ok(
                    ctx.call_id,
                    json!({
                        "room_id": room_id_str,
                        "decision": decision,
                        "record": record.map(|r| json!({
                            "status": r.status,
                            "transcript": serde_json::from_str::<serde_json::Value>(&r.transcript).ok(),
                            "decision_json": serde_json::from_str::<serde_json::Value>(&r.decision).ok(),
                            "created_at": r.created_at,
                        })),
                    }),
                )
                .with_actor(actor)
                .with_name("mind:conclave"),
            )
            .await;

        Ok(())
    }
}

pub struct MindAutonomy;

impl MindAutonomy {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for MindAutonomy {
    fn name(&self) -> &'static str {
        "mind:autonomy"
    }

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

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();

        let wake_mode = data
            .get("wake_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("normal")
            .trim();
        let wake_mode = match wake_mode {
            "init" => WakeMode::Init,
            _ => WakeMode::Normal,
        };

        let room_id = match data.get("room_id").and_then(|v| v.as_str()) {
            Some(s) => match Uuid::parse_str(s) {
                Ok(id) => id,
                Err(_) => {
                    tracing::warn!(provided = %s, "invalid room_id UUID, generating new one");
                    Uuid::new_v4()
                }
            },
            None => Uuid::new_v4(),
        };
        let room_id_str = room_id.to_string();

        let actor = ctx.actor.clone().unwrap_or_else(|| "system/mind".to_string());
        let trace = ConclaveTrace::new(ctx.call_id, tx.clone(), actor.clone());

        let _ = tx
            .send(
                Frame::event(
                    ctx.call_id,
                    json!({
                        "kind": "mind:start",
                        "room_id": room_id_str,
                        "type": "autonomy",
                        "scope": scope,
                        "wake_mode": format!("{:?}", wake_mode),
                    }),
                )
                .with_actor(actor.clone())
                .with_name("mind:autonomy"),
            )
            .await;

        let scopes = vec![crate::Scope::from(scope)];
        let conclave = Conclave::new(store.clone(), scopes, k.workspace().to_path_buf());
        let decision = conclave
            .autonomy_with_trace(&room_id_str, wake_mode, Some(trace))
            .await;

        let record = store.get_conclave(&room_id_str).ok().flatten();

        let _ = tx
            .send(
                Frame::ok(
                    ctx.call_id,
                    json!({
                        "room_id": room_id_str,
                        "decision": decision,
                        "record": record.map(|r| json!({
                            "status": r.status,
                            "transcript": serde_json::from_str::<serde_json::Value>(&r.transcript).ok(),
                            "decision_json": serde_json::from_str::<serde_json::Value>(&r.decision).ok(),
                            "created_at": r.created_at,
                        })),
                    }),
                )
                .with_actor(actor)
                .with_name("mind:autonomy"),
            )
            .await;

        Ok(())
    }
}

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(MindConclave::new()));
    dispatcher.register(Arc::new(MindAutonomy::new()));
}
