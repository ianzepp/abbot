use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::history::ToolRegistryTool;
use crate::kernel::{Frame, KernelError, KernelDispatcher, Syscall, SyscallContext};
use crate::runtime::Kernel;

async fn deliver_result(
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

    let tool_call_id = data
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if tool_call_id.is_empty() {
        return Err(KernelError::invalid_args("tool_call_id is required"));
    }

    let output = data
        .get("output")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    k.external_tools()
        .deliver_result(scope, tool_call_id, output)
        .await
        .map_err(KernelError::invalid_args)?;

    k.bump_activity();
    let _ = tx
        .send(Frame::ok(ctx.call_id, json!({"delivered": true})))
        .await;
    Ok(())
}

pub struct ToolResult;

impl ToolResult {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolResult {
    fn name(&self) -> &'static str {
        "tool:result"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        deliver_result(ctx, data, tx).await
    }
}

// Back-compat alias; prefer tool:result.
pub struct ToolDeliverResult;

impl ToolDeliverResult {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolDeliverResult {
    fn name(&self) -> &'static str {
        "tool:deliver_result"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        deliver_result(ctx, data, tx).await
    }
}

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
            .send(Frame::ok(ctx.call_id, json!({"registered": true, "count": out.len()})))
            .await;
        Ok(())
    }
}

pub fn register(dispatcher: &mut KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(ToolResult::new()));
    dispatcher.register(Arc::new(ToolDeliverResult::new()));
    dispatcher.register(Arc::new(ToolRegister::new()));
}
