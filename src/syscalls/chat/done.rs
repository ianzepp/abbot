use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_scope};

pub struct ChatDone;

#[async_trait]
impl Syscall for ChatDone {
    fn name(&self) -> &'static str {
        "chat:done"
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
            .unwrap_or("")
            .trim();
        if reason != "complete" && reason != "awaiting_tools" {
            return Err(KernelError::invalid_args("invalid reason"));
        }

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::item(
                    ctx.call_id,
                    json!({"type": "done", "reason": reason}),
                )
                .with_name("chat:done")
                .with_actor(ctx.actor_str().to_string()),
            )
            .await;
        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::done(ctx.call_id)
                    .with_name("chat:done")
                    .with_actor(ctx.actor_str().to_string()),
            )
            .await;
        k.sigcalls().close(scope, reply_to).await;

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"closed": true})))
            .await;
        Ok(())
    }
}
