//! Want:List - Retrieve wants from EMS-backed queue
//!
//! Queries the EMS `wants` table for pending wants, sorted by priority then creation time.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct WantListArgs {
    #[serde(default)]
    limit: Option<usize>,
}

pub struct WantList;

impl Default for WantList {
    fn default() -> Self {
        Self::new()
    }
}

impl WantList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for WantList {
    fn name(&self) -> &'static str {
        "want:list"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
        };

        let args: WantListArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let limit = args.limit.unwrap_or(20).clamp(1, 100);

        let items = {
            let ems = ems.lock().await;
            ems.select(
                "wants",
                Some(&json!({"status": "pending"})),
                None,
                Some(&json!(["priority ASC", "created_at ASC"])),
                Some(limit),
                None,
            )
            .await
            .map_err(|e| KernelError::io(format!("failed to list wants: {e}")))?
        };

        let out: Vec<_> = items
            .into_iter()
            .map(|w| {
                json!({
                    "id": w.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    "want": w.get("want").and_then(|v| v.as_str()).unwrap_or(""),
                    "context": w.get("context").and_then(|v| v.as_str()).unwrap_or(""),
                    "priority": w.get("priority").and_then(|v| v.as_str()).unwrap_or("normal"),
                    "source": w.get("source").and_then(|v| v.as_str()).unwrap_or("mind"),
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"wants": out, "count": out.len()}),
            ))
            .await;

        Ok(())
    }
}
