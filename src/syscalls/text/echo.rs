use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

#[derive(Debug, Deserialize)]
struct TextEchoArgs {
    text: String,
}

pub struct TextEcho;

impl TextEcho {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TextEcho {
    fn name(&self) -> &'static str {
        "text:echo"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: TextEchoArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"text": args.text})))
            .await;

        Ok(())
    }
}
