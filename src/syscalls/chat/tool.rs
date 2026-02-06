use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TurnKey};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_scope};

pub struct ChatTool;

#[async_trait]
impl Syscall for ChatTool {
    fn name(&self) -> &'static str {
        "chat:tool"
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
        let arguments = data.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
        if !arguments.is_object() {
            return Err(KernelError::invalid_args("arguments must be an object"));
        }

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let key = TurnKey::new(scope, reply_to);
        k.turns()
            .register_external_tool(&key, tool_call_id, name)
            .await
            .map_err(KernelError::invalid_args)?;

        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::item(
                    ctx.call_id,
                    json!({
                        "type": "tool_call",
                        "tool_call_id": tool_call_id,
                        "name": name,
                        "arguments": arguments,
                    }),
                )
                .with_name("chat:tool")
                .with_actor(ctx.actor_str().to_string()),
            )
            .await;

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"sent": true}))).await;
        Ok(())
    }
}
