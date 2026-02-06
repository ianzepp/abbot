use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, NeedKernel, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct NeedEnqueue;

impl NeedEnqueue {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedEnqueue {
    fn name(&self) -> &'static str {
        "need:enqueue"
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
        let need = NeedKernel::need_from_json(data).map_err(|e| KernelError::invalid_args(e))?;
        k.needs().enqueue(need).await;
        k.bump_activity();
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"enqueued": true})))
            .await;
        Ok(())
    }
}
