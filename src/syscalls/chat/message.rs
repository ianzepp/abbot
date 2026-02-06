use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

use super::{log_chat, parse_reply_to, parse_scope};

pub struct ChatMessage;

#[async_trait]
impl Syscall for ChatMessage {
    fn name(&self) -> &'static str {
        "chat:message"
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
        let content = data
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if content.trim().is_empty() {
            return Err(KernelError::invalid_args("content is required"));
        }

        let actor = ctx.actor_str();

        if actor.starts_with("head/") {
            log_chat(scope, "chat:head", &content, reply_to, actor).await;

            let Some(k) = Kernel::get() else {
                return Err(KernelError::internal("kernel not initialized"));
            };
            k.sigcalls()
                .send(
                    scope,
                    reply_to,
                    Frame::item(
                        ctx.call_id,
                        json!({"type": "text_delta", "content": content}),
                    )
                    .with_name("chat:message")
                    .with_actor(actor.to_string()),
                )
                .await;
        } else if actor == "user" || actor.starts_with("human/") {
            log_chat(scope, "chat:user", &content, reply_to, actor).await;

            let Some(k) = Kernel::get() else {
                return Err(KernelError::internal("kernel not initialized"));
            };
            let dispatcher = k.dispatcher().await;
            let need_id = Uuid::new_v4().to_string();
            let req = Frame::req(
                "need:enqueue",
                json!({
                    "need_id": need_id,
                    "source": "user",
                    "priority": "normal",
                    "need": content,
                    "context": "",
                    "scope": scope,
                    "reply_to": reply_to.to_string(),
                    "reconvene": false,
                }),
            )
            .with_actor(actor.to_string());
            let mut rx = dispatcher.dispatch(
                req,
                k.workspace().to_path_buf(),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx.recv().await;
        } else {
            return Err(KernelError::invalid_args(
                "chat:message actor must be user or head/*",
            ));
        }

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"sent": true}))).await;
        Ok(())
    }
}
