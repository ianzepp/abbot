use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct RoomReschedule;

impl RoomReschedule {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomReschedule {
    fn name(&self) -> &'static str {
        "room:reschedule"
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

        let run_after_ms = data
            .get("run_after_ms")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| KernelError::invalid_args("run_after_ms is required"))?;

        let updated = store
            .reschedule_room_schedule(id, run_after_ms)
            .map_err(|e| KernelError::internal(format!("failed to reschedule: {}", e)))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"id": id, "updated": updated, "run_after_ms": run_after_ms}),
            ))
            .await;
        Ok(())
    }
}
