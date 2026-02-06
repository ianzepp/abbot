use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_scope};

pub struct ChatError;

#[async_trait]
impl Syscall for ChatError {
    fn name(&self) -> &'static str {
        "chat:error"
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
        let code = data
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if code.is_empty() {
            return Err(KernelError::invalid_args("code is required"));
        }
        let message = data
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if message.is_empty() {
            return Err(KernelError::invalid_args("message is required"));
        }

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::error(
                    ctx.call_id,
                    json!({"code": code, "message": message}),
                )
                .with_name("chat:error")
                .with_actor(ctx.actor_str().to_string()),
            )
            .await;
        k.sigcalls().close(scope, reply_to).await;

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"closed": true}))).await;
        Ok(())
    }
}
