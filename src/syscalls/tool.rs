use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

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
        ctx.check_cancelled()?;
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        let tool_call_id = data
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if tool_call_id.is_empty() {
            return Err(KernelError::invalid_args("tool_call_id is required"));
        }

        let output = data
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        k.external_tools()
            .deliver_result(scope, tool_call_id, output)
            .await
            .map_err(KernelError::invalid_args)?;

        k.bump_activity();
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"delivered": true})))
            .await;
        Ok(())
    }
}

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(ToolDeliverResult::new()));
}
