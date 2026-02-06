use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TaskStatus};
use crate::runtime::Kernel;

pub struct TaskStatusGet;

impl TaskStatusGet {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskStatusGet {
    fn name(&self) -> &'static str {
        "task:status"
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

        let status = k.tasks().status(task_id).await;
        let out = match status {
            None => json!({"exists": false}),
            Some(TaskStatus::Queued) => json!({"exists": true, "status": "queued"}),
            Some(TaskStatus::Running { hand_id, .. }) => {
                json!({"exists": true, "status": "running", "hand_id": hand_id})
            }
            Some(TaskStatus::Done { ok, summary, .. }) => {
                json!({"exists": true, "status": "done", "ok": ok, "summary": summary})
            }
        };

        let _ = tx.send(Frame::ok(ctx.call_id, out)).await;
        Ok(())
    }
}
