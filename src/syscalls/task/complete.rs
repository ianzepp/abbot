use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct TaskComplete;

impl TaskComplete {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskComplete {
    fn name(&self) -> &'static str {
        "task:complete"
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

        let task_id = data
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err(KernelError::invalid_args("task_id is required"));
        }

        let ok = data.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let summary = data
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        k.tasks().complete(task_id, ok, summary).await;
        k.bump_activity();
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"updated": true})))
            .await;
        Ok(())
    }
}
