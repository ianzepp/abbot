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
        let need_id = data
            .get("need_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if need_id.is_empty() {
            return Err(KernelError::invalid_args("need_id is required"));
        }
        let existed = k.needs().fulfill(need_id).await.is_some();
        k.bump_activity();
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"fulfilled": existed})))
            .await;
        Ok(())
    }
}
