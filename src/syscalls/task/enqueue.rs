use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TaskKernel};
use crate::runtime::Kernel;

pub struct TaskEnqueue;

impl TaskEnqueue {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskEnqueue {
    fn name(&self) -> &'static str {
        "task:enqueue"
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

        let task = TaskKernel::task_from_json(data).map_err(KernelError::invalid_args)?;
        let task_id = task.id.clone();
        k.tasks().enqueue(task).await;
        k.bump_activity();

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"task_id": task_id})))
            .await;
        Ok(())
    }
}
