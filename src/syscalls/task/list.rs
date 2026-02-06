use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

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

        let args: TaskListArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let status_filter = args.status.as_deref().unwrap_or("all");
        let limit = args.limit.unwrap_or(50).clamp(1, 100);

        // Task listing is kernel-owned; this is a placeholder that returns empty.
        let tasks: Vec<serde_json::Value> = Vec::new();
        let _ = limit;
        let _ = status_filter;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "tasks": tasks,
                    "count": tasks.len(),
                    "status_filter": status_filter
                }),
            ))
            .await;

        Ok(())
    }
}
