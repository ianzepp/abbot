use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::history::ToolRegistryTool;
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct ToolRegister;

impl ToolRegister {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolRegister {
    fn name(&self) -> &'static str {
        "tool:register"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        let tools = data
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| KernelError::invalid_args("tools must be an array"))?;

        let mut out: Vec<ToolRegistryTool> = Vec::new();
        for t in tools {
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            if name.is_empty() {
                return Err(KernelError::invalid_args("tool name is required"));
            }
            let summary = t
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = t
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let schema_json = t
                .get("schema_json")
                .and_then(|v| v.as_str())
                .unwrap_or("null")
                .to_string();

            out.push(ToolRegistryTool {
                name: name.to_string(),
                summary,
                description,
                schema_json,
            });
        }

        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        store
            .replace_external_tools(scope, &out)
            .map_err(|e| KernelError::internal(format!("failed to persist tool registry: {e}")))?;

        k.external_tools().replace_tools(scope, &out).await;
        k.bump_activity();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"registered": true, "count": out.len()}),
            ))
            .await;
        Ok(())
    }
}
