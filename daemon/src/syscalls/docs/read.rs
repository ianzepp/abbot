//! Docs:Read — retrieve full content of a specific document by name.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::build_catalog;

pub struct DocsRead;

impl Default for DocsRead {
    fn default() -> Self {
        Self::new()
    }
}

impl DocsRead {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for DocsRead {
    fn name(&self) -> &'static str {
        "docs:read"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if name.is_empty() {
            return Err(KernelError::invalid_args("name is required"));
        }

        let catalog = build_catalog();

        match catalog.iter().find(|d| d.name == name) {
            Some(d) => {
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({
                            "name": d.name,
                            "content": d.content,
                            "size": d.content.len(),
                        }),
                    ))
                    .await;
                Ok(())
            }
            None => {
                let available: Vec<&str> = catalog.iter().map(|d| d.name.as_str()).collect();
                Err(KernelError::not_found(format!(
                    "document '{}' not found (available: {})",
                    name,
                    available.join(", ")
                )))
            }
        }
    }
}
