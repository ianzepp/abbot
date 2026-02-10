//! Chat:Status - Emit real-time activity feedback during agent turns
//!
//! Surfaces two types of feedback to connected clients:
//! 1. **Thinking** — transient indicator while LLM is inferring
//! 2. **Tool** — persistent activity line showing internal tool dispatch

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_room};

pub struct ChatStatus;

#[async_trait]
impl Syscall for ChatStatus {
    fn name(&self) -> &'static str {
        "chat:status"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let room = parse_room(&data)?;
        let reply_to = parse_reply_to(&data)?;

        let status = data
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if status != "thinking" && status != "tool" && status != "thought" {
            return Err(KernelError::invalid_args(
                "status must be \"thinking\", \"tool\", or \"thought\"",
            ));
        }

        let actor = data
            .get("actor")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tool = data
            .get("tool")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let summary = data
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let mut payload = json!({
            "type": "status",
            "status": status,
        });
        if !actor.is_empty() {
            payload["actor"] = json!(actor);
        }
        if !tool.is_empty() {
            payload["tool"] = json!(tool);
        }
        if !summary.is_empty() {
            payload["summary"] = json!(summary);
        }
        let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if !content.is_empty() {
            payload["content"] = json!(content);
        }

        k.sigcalls()
            .send(
                room,
                reply_to,
                Frame::item(ctx.call_id, payload)
                    .with_name("chat:status")
                    .with_actor(ctx.actor_str().to_string()),
            )
            .await;

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"sent": true}))).await;
        Ok(())
    }
}
