use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, RoomKind, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct RoomSchedule;

impl RoomSchedule {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomSchedule {
    fn name(&self) -> &'static str {
        "room:schedule"
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

        let room_type = data
            .get("room_type")
            .or_else(|| data.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if RoomKind::from_str(room_type).is_none() {
            return Err(KernelError::invalid_args(
                "room_type must be 'conclave', 'autonomy', or 'work'",
            ));
        }

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();

        let run_after_ms = data
            .get("run_after_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64
            });

        let reason = data
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("scheduled")
            .trim();

        let wake_mode = data
            .get("wake_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("normal")
            .trim();

        let constraints_json = data
            .get("constraints")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "{}".to_string());

        let context = data
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let id = Uuid::new_v4().to_string();

        store
            .insert_room_schedule(
                &id,
                room_type,
                scope,
                run_after_ms,
                reason,
                wake_mode,
                &constraints_json,
                context,
            )
            .map_err(|e| KernelError::internal(format!("failed to insert schedule: {}", e)))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"schedule_id": id, "room_type": room_type, "scope": scope, "run_after_ms": run_after_ms}),
            ))
            .await;
        Ok(())
    }
}
