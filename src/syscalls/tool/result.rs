use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::deliver_result;

pub struct ToolResult;

impl ToolResult {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolResult {
    fn name(&self) -> &'static str {
        "tool:result"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        deliver_result(ctx, data, tx).await
    }
}
