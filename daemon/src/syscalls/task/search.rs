//! Task:Search - Search tasks by pattern via EMS query

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct TaskSearchArgs {
    pattern: String,
    #[serde(default)]
    limit: Option<usize>,
}

pub struct TaskSearch;

impl TaskSearch {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskSearch {
    fn name(&self) -> &'static str {
        "task:search"
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

        let args: TaskSearchArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let pattern = args.pattern.trim();
        if pattern.is_empty() {
            return Err(KernelError::invalid_args("pattern is required"));
        }

        let limit = args.limit.unwrap_or(20).clamp(1, 50);
        let like_pattern = format!("%{}%", pattern);

        let matches = {
            let ems = ems.lock().await;
            ems.query(
                "SELECT id, status, scope, prompt FROM \"tasks\" WHERE \"prompt\" LIKE ?1 ORDER BY \"created_at\" DESC LIMIT ?2",
                &[json!(like_pattern), json!(limit as i64)],
            )
            .await
            .map_err(|e| KernelError::io(format!("failed to search tasks: {e}")))?
        };

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "matches": matches,
                    "count": matches.len(),
                    "pattern": pattern,
                }),
            ))
            .await;

        Ok(())
    }
}
