use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelDispatcher, KernelError, Syscall, SyscallContext};

struct Doc {
    name: &'static str,
    content: &'static str,
}

const DOCS: &[Doc] = &[
    Doc {
        name: "architecture",
        content: include_str!("../docs/architecture.md"),
    },
    Doc {
        name: "syscalls",
        content: include_str!("../docs/syscalls.md"),
    },
    Doc {
        name: "tools",
        content: include_str!("../docs/tools.md"),
    },
];

// =============================================================================
// DOCS:LIST — List available document names and sizes
// =============================================================================

pub struct DocsList;

impl DocsList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for DocsList {
    fn name(&self) -> &'static str {
        "docs:list"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let docs: Vec<serde_json::Value> = DOCS
            .iter()
            .map(|d| {
                json!({
                    "name": d.name,
                    "size": d.content.len(),
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({ "docs": docs })))
            .await;

        Ok(())
    }
}

// =============================================================================
// DOCS:SEARCH — Case-insensitive text search across all docs
// =============================================================================

pub struct DocsSearch;

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
            .unwrap_or(20)
            .clamp(1, 50) as usize;

        let query_lower = query.to_lowercase();
        let mut count: usize = 0;

        for doc in DOCS {
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

// =============================================================================
// DOCS:READ — Read a specific document by name
// =============================================================================

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

// =============================================================================
// REGISTRATION
// =============================================================================

pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(DocsList::new()));
    dispatcher.register(Arc::new(DocsSearch::new()));
    dispatcher.register(Arc::new(DocsRead::new()));
}
