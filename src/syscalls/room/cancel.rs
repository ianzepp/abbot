use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct RoomCancel;

impl RoomCancel {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomCancel {
    fn name(&self) -> &'static str {
        "room:cancel"
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

        let id = data
            .get("id")
            .or_else(|| data.get("schedule_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::invalid_args("id is required"))?;

        let cancelled = store
            .cancel_room_schedule(id)
            .map_err(|e| KernelError::internal(format!("failed to cancel: {}", e)))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"id": id, "cancelled": cancelled}),
            ))
            .await;
        Ok(())
    }
}
