//! Task:Status - Query current state of tasks from EMS
//!
//! Reads task status from the EMS `tasks` table.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
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

        let row = {
            let ems = ems.lock().await;
            let rows = ems
                .select(
                    "tasks",
                    Some(&json!({"id": task_id})),
                    None,
                    None,
                    Some(1),
                    None,
                )
                .await
                .map_err(|e| KernelError::io(format!("failed to query task: {e}")))?;
            rows.into_iter().next()
        };

        let out = match row {
            None => json!({"exists": false}),
            Some(r) => {
                let status = r.get("status").and_then(|v| v.as_str()).unwrap_or("pending");
                match status {
                    "pending" => json!({"exists": true, "status": "queued"}),
                    "running" => {
                        let hand_id = r.get("lease_owner").and_then(|v| v.as_str()).unwrap_or("");
                        json!({"exists": true, "status": "running", "hand_id": hand_id})
                    }
                    "completed" => {
                        let summary = r.get("result").and_then(|v| v.as_str()).unwrap_or("");
                        json!({"exists": true, "status": "done", "ok": true, "summary": summary})
                    }
                    "failed" => {
                        let error = r.get("error").and_then(|v| v.as_str()).unwrap_or("");
                        json!({"exists": true, "status": "done", "ok": false, "summary": error})
                    }
                    other => json!({"exists": true, "status": other}),
                }
            }
        };

        let _ = tx.send(Frame::ok(ctx.call_id, out)).await;
        Ok(())
    }
}
