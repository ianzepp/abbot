//! Docs:Search — full-text search across all documentation.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::build_catalog;

const DEFAULT_LIMIT: u64 = 20;
const MAX_LIMIT: u64 = 50;

pub struct DocsSearch;

impl Default for DocsSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl DocsSearch {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for DocsSearch {
    fn name(&self) -> &'static str {
        "docs:search"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let query = data
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if query.is_empty() {
            return Err(KernelError::invalid_args("query is required"));
        }

        let limit = data
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT) as usize;

        let query_lower = query.to_lowercase();
        let catalog = build_catalog();
        let mut count: usize = 0;

        for doc in &catalog {
            if count >= limit {
                break;
            }

            for (line_num, line) in doc.content.lines().enumerate() {
                if count >= limit {
                    break;
                }

                if line.to_lowercase().contains(&query_lower) {
                    let _ = tx
                        .send(Frame::item(
                            ctx.call_id,
                            json!({
                                "doc": doc.name,
                                "line": line_num + 1,
                                "text": line,
                            }),
                        ))
                        .await;
                    count += 1;
                }
            }
        }

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({ "count": count })))
            .await;

        Ok(())
    }
}
