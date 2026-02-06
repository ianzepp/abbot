use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, RoomKind, Syscall, SyscallContext};
use crate::runtime::Kernel;

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
                "room type must be 'conclave', 'autonomy', or 'work'",
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
