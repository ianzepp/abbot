//! Task:List - List tasks from EMS with optional status filtering

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct TaskListArgs {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

pub struct TaskList;

impl TaskList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskList {
    fn name(&self) -> &'static str {
        "task:list"
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
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
        };

        let args: TaskListArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let status_filter = args.status.as_deref().unwrap_or("all");
        let limit = args.limit.unwrap_or(50).clamp(1, 100);

        let where_clause = match status_filter {
            "all" => None,
            "queued" => Some(json!({"status": "pending"})),
            "running" => Some(json!({"status": "running"})),
            "done" => Some(json!({"status": {"$in": ["completed", "failed"]}})),
            other => Some(json!({"status": other})),
        };

        let tasks = {
            let ems = ems.lock().await;
            ems.select(
                "tasks",
                where_clause.as_ref(),
                None,
                Some(&json!(["created_at DESC"])),
                Some(limit),
                None,
            )
            .await
            .map_err(|e| KernelError::io(format!("failed to list tasks: {e}")))?
        };

        let out: Vec<_> = tasks
            .into_iter()
            .map(|t| {
                json!({
                    "id": t.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    "status": t.get("status").and_then(|v| v.as_str()).unwrap_or("pending"),
                    "scope": t.get("scope").and_then(|v| v.as_str()).unwrap_or("main"),
                    "prompt": t.get("prompt").and_then(|v| v.as_str()).unwrap_or(""),
                    "head_id": t.get("head_id").and_then(|v| v.as_str()).unwrap_or(""),
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "tasks": out,
                    "count": out.len(),
                    "status_filter": status_filter,
                }),
            ))
            .await;

        Ok(())
    }
}
