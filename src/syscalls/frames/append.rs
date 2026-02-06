use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

pub struct FramesAppend;

impl FramesAppend {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for FramesAppend {
    fn name(&self) -> &'static str {
        "frames:append"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let kind = data
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("log")
            .trim();
        if kind.is_empty() {
            return Err(KernelError::invalid_args("kind is required"));
        }

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        let payload = json!({
            "kind": kind,
            "scope": scope,
            "data": data.get("data").cloned().unwrap_or(serde_json::Value::Null),
        });

        let _ = tx.send(Frame::event(ctx.call_id, payload)).await;
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"appended": true})))
            .await;
        Ok(())
    }
}
