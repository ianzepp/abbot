use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct TaskReadArgs {
    task_id: String,
}

pub struct TaskRead;

impl TaskRead {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskRead {
    fn name(&self) -> &'static str {
        "task:read"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        let args: TaskReadArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let task_id = args.task_id.trim();
        if task_id.is_empty() {
            return Err(KernelError::invalid_args("task_id is required"));
        }

        let execs = store
            .get_hand_execs(task_id)
            .map_err(|e| KernelError::io(format!("query error: {e}")))?;

        if execs.is_empty() {
            return Err(KernelError::not_found(format!("task not found: {}", task_id)));
        }

        let logs: Vec<_> = execs
            .iter()
            .map(|e| {
                let output: String = e.output.chars().take(500).collect();
                json!({
                    "step": e.step,
                    "tool": e.tool,
                    "success": e.success,
                    "output": output
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "id": task_id,
                    "status": "completed",
                    "execution_log": logs,
                    "steps": logs.len()
                }),
            ))
            .await;

        Ok(())
    }
}
