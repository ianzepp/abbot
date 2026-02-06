//! Task:Enqueue - Add work items to EMS-backed task queue
//!
//! Inserts a row into the EMS `tasks` table and wakes one waiting leaser.

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
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
        };

        // Parse task fields (reuse existing validation)
        let task = TaskKernel::task_from_json(data).map_err(KernelError::invalid_args)?;

        let batch_calls_json = task.batch_calls.as_ref().map(|calls| {
            let arr: Vec<serde_json::Value> = calls
                .iter()
                .map(|c| json!({"name": c.name, "args": c.args}))
                .collect();
            serde_json::to_string(&arr).unwrap_or_default()
        });

        let row = json!({
            "id": task.id,
            "status": "pending",
            "scope": task.scope,
            "prompt": task.prompt,
            "input": task.input,
            "head_id": task.head_id,
            "notify_scope": task.notify_scope,
            "reply_to": task.reply_to.map(|u| u.to_string()),
            "batch_calls": batch_calls_json,
        });

        {
            let mut ems = ems.lock().unwrap();
            ems.insert("tasks", &row)
                .map_err(|e| KernelError::io(format!("failed to enqueue task: {e}")))?;
        }

        // Set initial watcher state and wake leaser
        k.tasks().notify_enqueue();
        k.bump_activity();

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"task_id": task.id})))
            .await;
        Ok(())
    }
}
