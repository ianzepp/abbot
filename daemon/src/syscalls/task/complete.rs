//! Task:Complete - Mark leased task as finished via EMS update
//!
//! Updates task status to "completed" or "failed" in the EMS `tasks` table.

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
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
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

        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let status = if ok { "completed" } else { "failed" };

        {
            let mut ems = ems.lock().await;
            ems.update(
                "tasks",
                &json!({"id": task_id}),
                &json!({
                    "status": status,
                    "result": if ok { &summary } else { "" },
                    "error": if !ok { &summary } else { "" },
                    "completed_at": now,
                    "updated_at": now,
                }),
            )
            .await
            .map_err(|e| KernelError::io(format!("failed to complete task: {e}")))?;
        }

        // Notify watchers that task status changed
        k.tasks().notify_watcher(task_id).await;
        k.bump_activity();

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"updated": true})))
            .await;
        Ok(())
    }
}
