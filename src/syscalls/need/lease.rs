use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct NeedLease;

impl NeedLease {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedLease {
    fn name(&self) -> &'static str {
        "need:lease"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let need = tokio::select! {
            _ = ctx.cancel.cancelled() => {
                return Err(KernelError::cancelled("operation cancelled"));
            }
            n = k.needs().lease() => n,
        };

        k.bump_activity();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "need_id": need.id,
                    "source": need.source,
                    "priority": format!("{:?}", need.priority).to_ascii_lowercase(),
                    "need": need.need,
                    "context": need.context,
                    "scope": need.scope,
                    "reply_to": need.reply_to.map(|u| u.to_string()),
                    "reconvene": need.reconvene,
                }),
            ))
            .await;
        Ok(())
    }
}
