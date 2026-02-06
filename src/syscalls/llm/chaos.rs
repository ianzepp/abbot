use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

pub struct LlmChaos;

impl LlmChaos {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LlmChaos {
    fn name(&self) -> &'static str {
        "llm:chaos"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let pinned: std::collections::HashMap<String, String> = data
            .get("pin")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let exclude: Vec<String> = data
            .get("exclude")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let result = crate::runtime::chaos::roll(&pinned, &exclude);

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "traits": result.selections,
                    "prompt": result.prompt,
                }),
            ))
            .await;

        Ok(())
    }
}
