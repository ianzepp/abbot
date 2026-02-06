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
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        let args: WantListArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let limit = args.limit.unwrap_or(20).clamp(1, 100);

        let wants = store
            .list_wants(limit)
            .map_err(|e| KernelError::io(format!("failed to list wants: {e}")))?;

        let items: Vec<_> = wants
            .into_iter()
            .map(|w| {
                json!({
                    "id": w.id,
                    "want": w.want,
                    "context": w.context,
                    "priority": w.priority,
                    "source": w.source
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"wants": items, "count": items.len()}),
            ))
            .await;

        Ok(())
    }
}
