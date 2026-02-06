//! Want:Remove - Soft-delete want from EMS-backed queue
//!
//! Updates want status to "removed" (soft delete) instead of hard deleting.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct WantRemoveArgs {
    id: String,
}

pub struct WantRemove;

impl Default for WantRemove {
    fn default() -> Self {
        Self::new()
    }
}

impl WantRemove {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for WantRemove {
    fn name(&self) -> &'static str {
        "want:remove"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
        };

        let args: WantRemoveArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let rows_updated = {
            let mut ems = ems.lock().await;
            ems.update(
                "wants",
                &json!({"id": args.id, "status": "pending"}),
                &json!({
                    "status": "removed",
                    "updated_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
                }),
            )
            .await
            .map_err(|e| KernelError::io(format!("failed to remove want: {e}")))?
        };

        if rows_updated > 0 {
            let _ = tx
                .send(Frame::ok(ctx.call_id, json!({"removed": true})))
                .await;
        } else {
            let _ = tx
                .send(Frame::ok(
                    ctx.call_id,
                    json!({"removed": false, "reason": "not found"}),
                ))
                .await;
        }

        Ok(())
    }
}
