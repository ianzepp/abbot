use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::DOCS;

pub struct DocsList;

impl DocsList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for DocsList {
    fn name(&self) -> &'static str {
        "docs:list"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let docs: Vec<serde_json::Value> = DOCS
            .iter()
            .map(|d| {
                json!({
                    "name": d.name,
                    "size": d.content.len(),
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({ "docs": docs })))
            .await;

        Ok(())
    }
}
