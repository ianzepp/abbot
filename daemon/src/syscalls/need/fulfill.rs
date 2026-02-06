//! Need:Fulfill - Mark leased need as complete via EMS update
//!
//! Updates need status from "running" to "fulfilled" in the EMS `needs` table.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct NeedFulfill;

impl NeedFulfill {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedFulfill {
    fn name(&self) -> &'static str {
        "need:fulfill"
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
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
        };

        let need_id = data
            .get("need_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if need_id.is_empty() {
            return Err(KernelError::invalid_args("need_id is required"));
        }

        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

        let rows_updated = {
            let mut ems = ems.lock().await;
            ems.update(
                "needs",
                &json!({"id": need_id, "status": "running"}),
                &json!({
                    "status": "fulfilled",
                    "fulfilled_at": now,
                    "updated_at": now,
                }),
            )
            .await
            .map_err(|e| KernelError::io(format!("failed to fulfill need: {e}")))?
        };

        k.bump_activity();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"fulfilled": rows_updated > 0}),
            ))
            .await;
        Ok(())
    }
}
