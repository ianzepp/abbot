use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

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

        let args: TaskSearchArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let pattern = args.pattern.trim();
        if pattern.is_empty() {
            return Err(KernelError::invalid_args("pattern is required"));
        }

        let limit = args.limit.unwrap_or(20).clamp(1, 50);

        // Task search is a placeholder — live task search removed.
        let matches: Vec<serde_json::Value> = Vec::new();
        let _ = limit;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "matches": matches,
                    "count": matches.len(),
                    "pattern": pattern
                }),
            ))
            .await;

        Ok(())
    }
}
