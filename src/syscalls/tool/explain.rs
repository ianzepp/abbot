use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct ToolExplainArgs {
    name: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

pub struct ToolExplain;

impl ToolExplain {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolExplain {
    fn name(&self) -> &'static str {
        "tool:explain"
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
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        let args: ToolExplainArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let tool_name = args.name.trim();
        if tool_name.is_empty() {
            return Err(KernelError::invalid_args("name is required"));
        }

        let tool_name = tool_name.strip_prefix("user__").unwrap_or(tool_name);

        let scope = args
            .scope
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("main");

        let source = args
            .source
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("external");

        if source != "external" {
            return Err(KernelError::invalid_args("unsupported source"));
        }

        match store.get_tool(scope, source, tool_name) {
            Ok(Some(t)) => {
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({
                            "scope": scope,
                            "source": source,
                            "name": t.name,
                            "summary": t.summary,
                            "description": t.description,
                            "schema_json": t.schema_json,
                        }),
                    ))
                    .await;
                Ok(())
            }
            Ok(None) => Err(KernelError::not_found(format!(
                "tool not found: {} (source={})",
                tool_name, source
            ))),
            Err(e) => Err(KernelError::io(format!("db error: {e}"))),
        }
    }
}
