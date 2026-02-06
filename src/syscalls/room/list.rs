use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct RoomList;

impl RoomList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomList {
    fn name(&self) -> &'static str {
        "room:list"
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

        let status = data.get("status").and_then(|v| v.as_str());
        let room_type = data
            .get("room_type")
            .or_else(|| data.get("type"))
            .and_then(|v| v.as_str());
        let limit = data
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;

        let schedules = store
            .list_room_schedules(status, room_type, limit)
            .map_err(|e| KernelError::internal(format!("failed to list schedules: {}", e)))?;

        let items: Vec<serde_json::Value> = schedules
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "room_type": s.room_type,
                    "scope": s.scope,
                    "status": s.status,
                    "run_after_ms": s.run_after_ms,
                    "reason": s.reason,
                    "wake_mode": s.wake_mode,
                    "context": s.context,
                    "attempts": s.attempts,
                    "last_error": s.last_error,
                    "created_at_ms": s.created_at_ms,
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"schedules": items, "count": items.len()}),
            ))
            .await;
        Ok(())
    }
}
