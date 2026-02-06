use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::recall::Search;

#[derive(Debug, Deserialize)]
struct MemoryRecallArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

pub struct MemoryRecall {
    search: Option<Arc<Search>>,
}

impl MemoryRecall {
    pub fn new() -> Self {
        Self { search: None }
    }

    pub fn with_search(search: Arc<Search>) -> Self {
        Self {
            search: Some(search),
        }
    }
}

#[async_trait]
impl Syscall for MemoryRecall {
    fn name(&self) -> &'static str {
        "memory:recall"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: MemoryRecallArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let query = args.query.trim();
        if query.is_empty() {
            return Err(KernelError::invalid_args("query is required"));
        }

        let Some(search) = &self.search else {
            return Err(KernelError::disabled("memory search is not available"));
        };

        let limit = args.limit.unwrap_or(5).clamp(1, 20);

        let results = search
            .query(query, limit)
            .await
            .map_err(|e| KernelError::io(format!("recall error: {e}")))?;

        let out: Vec<serde_json::Value> = results
            .into_iter()
            .map(|r| {
                let content: String = r.content.chars().take(1200).collect();
                json!({
                    "distance": r.distance,
                    "source": r.source,
                    "file_path": r.file_path,
                    "project_path": r.project_path,
                    "content": content
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"results": out})))
            .await;

        Ok(())
    }
}
