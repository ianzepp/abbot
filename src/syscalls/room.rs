use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::kernel::{Frame, FrameOp, KernelError, RoomKind, Syscall, SyscallContext};
use crate::runtime::{Conclave, Kernel, WakeMode};

pub struct RoomCreate;

impl RoomCreate {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomCreate {
    fn name(&self) -> &'static str {
        "room:create"
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

        let kind = data
            .get("type")
            .or_else(|| data.get("kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let Some(kind) = RoomKind::from_str(kind) else {
            return Err(KernelError::invalid_args(
                "room type must be 'conclave' or 'autonomy'",
            ));
        };

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        let room_id = k.rooms().create(kind, scope).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"room_id": room_id.to_string(), "type": kind.as_str(), "scope": scope}),
            ))
            .await;
        Ok(())
    }
}

pub struct RoomStream;

impl RoomStream {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomStream {
    fn name(&self) -> &'static str {
        "room:stream"
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

        let room_id = data
            .get("room_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| KernelError::invalid_args("room_id is required"))?;

        if k.rooms().get(room_id).await.is_none() {
            return Err(KernelError::not_found("room not found"));
        }

        let rx = k.rooms().open_stream(room_id).await;
        let mut stream = ReceiverStream::new(rx);

        while let Some(mut frame) = stream.next().await {
            // Re-parent to this syscall call_id.
            frame.parent_id = Some(ctx.call_id);
            let is_terminal = matches!(
                frame.op,
                FrameOp::Ok | FrameOp::Error | FrameOp::Done | FrameOp::Redirect
            );
            let _ = tx.send(frame).await;
            if is_terminal {
                break;
            }
            if ctx.is_cancelled() {
                break;
            }
        }

        let _ = tx.send(Frame::done(ctx.call_id)).await;
        Ok(())
    }
}

pub struct RoomRun;

impl RoomRun {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomRun {
    fn name(&self) -> &'static str {
        "room:run"
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

        let room_id = data
            .get("room_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| KernelError::invalid_args("room_id is required"))?;

        let rec = k
            .rooms()
            .get(room_id)
            .await
            .ok_or_else(|| KernelError::not_found("room not found"))?;

        if !k.rooms().has_stream(room_id).await {
            return Err(KernelError::invalid_args(
                "room stream not opened (call room:stream before room:run)",
            ));
        }

        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        let wake_mode = data
            .get("wake_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("normal")
            .trim();
        let wake_mode = match wake_mode {
            "init" => WakeMode::Init,
            _ => WakeMode::Normal,
        };

        let context = data.get("context").and_then(|v| v.as_str()).unwrap_or("");

        // Emit start, run, then emit end with stored transcript/decision.
        let _ = k
            .rooms()
            .send(
                room_id,
                Frame::event(
                    room_id,
                    json!({"kind": "room_start", "room_id": room_id.to_string(), "type": rec.kind.as_str(), "context": context, "wake_mode": format!("{:?}", wake_mode)}),
                ),
            )
            .await;

        let scopes = vec![crate::Scope::from(rec.scope.as_str())];
        let conclave = Conclave::new(store.clone(), scopes, k.workspace().to_path_buf());
        let room_id_str = room_id.to_string();

        let decision = match rec.kind {
            RoomKind::Conclave => conclave.convene(&room_id_str, wake_mode).await,
            RoomKind::Autonomy => conclave.autonomy(&room_id_str, wake_mode).await,
        };

        let record = store.get_conclave(&room_id_str).ok().flatten();
        let _ = k
            .rooms()
            .send(
                room_id,
                Frame::event(
                    room_id,
                    json!({
                        "kind": "room_end",
                        "room_id": room_id_str,
                        "status": record.as_ref().map(|r| r.status.clone()).unwrap_or_else(|| if decision.is_some() {"done".to_string()} else {"no_decision".to_string()}),
                        "decision": decision,
                        "transcript": record.as_ref().and_then(|r| serde_json::from_str::<serde_json::Value>(&r.transcript).ok()),
                    }),
                ),
            )
            .await;

        let _ = k
            .rooms()
            .send(room_id, Frame::ok(room_id, json!({"status": "closed"})))
            .await;
        k.rooms().close_stream(room_id).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"room_id": room_id.to_string(), "status": "done", "decision": decision}),
            ))
            .await;
        Ok(())
    }
}

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(RoomCreate::new()));
    dispatcher.register(Arc::new(RoomStream::new()));
    dispatcher.register(Arc::new(RoomRun::new()));
}
