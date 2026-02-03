use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use crate::kernel::{Frame, KernelError, RoomKind, Syscall, SyscallContext};
use crate::runtime::{Conclave, Kernel, WakeMode};

pub struct MindConveneConclave;

impl MindConveneConclave {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for MindConveneConclave {
    fn name(&self) -> &'static str {
        "mind:convene_conclave"
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

        let room_id = k.rooms().create(RoomKind::Conclave, scope).await;
        // Optional: if the caller opened room:stream already, publish a start marker.
        let _ = k
            .rooms()
            .send(
                room_id,
                Frame::event(
                    room_id,
                    json!({"kind": "room_start", "room_id": room_id.to_string(), "type": "conclave"}),
                ),
            )
            .await;

        let scopes = vec![crate::Scope::from(scope)];
        let conclave = Conclave::new(store.clone(), scopes, k.workspace().to_path_buf());
        let decision = conclave.convene(&room_id.to_string(), wake_mode).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"room_id": room_id.to_string(), "decision": decision}),
            ))
            .await;
        Ok(())
    }
}

pub struct MindConveneAutonomy;

impl MindConveneAutonomy {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for MindConveneAutonomy {
    fn name(&self) -> &'static str {
        "mind:convene_autonomy"
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

        let room_id = k.rooms().create(RoomKind::Autonomy, scope).await;
        let scopes = vec![crate::Scope::from(scope)];
        let conclave = Conclave::new(store.clone(), scopes, k.workspace().to_path_buf());
        let decision = conclave.autonomy(&room_id.to_string(), wake_mode).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"room_id": room_id.to_string(), "decision": decision}),
            ))
            .await;
        Ok(())
    }
}

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(MindConveneConclave::new()));
    dispatcher.register(Arc::new(MindConveneAutonomy::new()));
}
