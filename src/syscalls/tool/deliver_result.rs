use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::deliver_result;

// Back-compat alias; prefer tool:result.
pub struct ToolDeliverResult;

impl ToolDeliverResult {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolDeliverResult {
    fn name(&self) -> &'static str {
        "tool:deliver_result"
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
