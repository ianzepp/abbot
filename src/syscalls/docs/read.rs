use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::DOCS;

pub struct DocsRead;

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

        let doc = DOCS.iter().find(|d| d.name == name);

        match doc {
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
                let available: Vec<&str> = DOCS.iter().map(|d| d.name).collect();
                Err(KernelError::not_found(format!(
                    "document '{}' not found (available: {})",
                    name,
                    available.join(", ")
                )))
            }
        }
    }
}
