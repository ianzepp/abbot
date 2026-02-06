use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TurnKey};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_scope};

pub struct ChatCancel;

#[async_trait]
impl Syscall for ChatCancel {
    fn name(&self) -> &'static str {
        "chat:cancel"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let scope = parse_scope(&data)?;
        let reply_to = parse_reply_to(&data)?;
        let reason = data
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("client_disconnect")
            .trim();

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let key = TurnKey::new(scope, reply_to);
        k.turns().cancel(&key, reason).await;

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"cancelled": true})))
            .await;
        Ok(())
    }
}
