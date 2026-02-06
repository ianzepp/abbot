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

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        let args: WantRemoveArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        match store.remove_want(&args.id) {
            Ok(true) => {
                let _ = tx
                    .send(Frame::ok(ctx.call_id, json!({"removed": true})))
                    .await;
                Ok(())
            }
            Ok(false) => {
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({"removed": false, "reason": "not found"}),
                    ))
                    .await;
                Ok(())
            }
            Err(e) => Err(KernelError::io(format!("failed to remove want: {e}"))),
        }
    }
}
