use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

pub struct LlmChaosList;

impl LlmChaosList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LlmChaosList {
    fn name(&self) -> &'static str {
        "llm:chaos:list"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let axes = crate::runtime::chaos::list_axes();
        let map: serde_json::Map<String, serde_json::Value> = axes
            .into_iter()
            .map(|(name, levels)| {
                (
                    name.to_string(),
                    json!(levels),
                )
            })
            .collect();

        let _ = tx
            .send(Frame::ok(ctx.call_id, serde_json::Value::Object(map)))
            .await;

        Ok(())
    }
}
