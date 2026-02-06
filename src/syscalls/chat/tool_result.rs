use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TurnKey};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_scope};

pub struct ChatToolResult;

#[async_trait]
impl Syscall for ChatToolResult {
    fn name(&self) -> &'static str {
        "chat:tool_result"
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
        let tool_call_id = data
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if tool_call_id.is_empty() {
            return Err(KernelError::invalid_args("tool_call_id is required"));
        }
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            return Err(KernelError::invalid_args("name is required"));
        }
        let content = data
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::invalid_args("content is required"))?
            .to_string();
        let is_error = data
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let key = TurnKey::new(scope, reply_to);
        let cancelled = k.turns().is_cancelled(&key).await;

        k.turns()
            .deliver_external_tool_result(&key, tool_call_id, name, content.clone(), is_error)
            .await
            .map_err(KernelError::invalid_args)?;

        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "log:append",
            json!({
                "kind": "chat:tool_result",
                "scope": scope,
                "data": {
                    "tool_call_id": tool_call_id,
                    "name": name,
                    "content": content,
                    "is_error": is_error,
                }
            }),
        )
        .with_actor(ctx.actor_str().to_string());
        let mut rx = dispatcher.dispatch(
            req,
            k.workspace().to_path_buf(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;

        if cancelled {
            return Err(KernelError::cancelled("turn cancelled"));
        }

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"delivered": true})))
            .await;
        Ok(())
    }
}
