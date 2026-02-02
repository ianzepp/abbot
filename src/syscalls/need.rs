use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, NeedKernel, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct NeedEnqueue;

impl NeedEnqueue {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedEnqueue {
    fn name(&self) -> &'static str {
        "need:enqueue"
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
        let need = NeedKernel::need_from_json(data)
            .map_err(|e| KernelError::invalid_args(e))?;
        k.needs().enqueue(need).await;
        let _ = tx.send(Frame::ok(ctx.call_id, json!({"enqueued": true}))).await;
        Ok(())
    }
}

pub struct NeedLease;

impl NeedLease {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedLease {
    fn name(&self) -> &'static str {
        "need:lease"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let need = tokio::select! {
            _ = ctx.cancel.cancelled() => {
                return Err(KernelError::cancelled("operation cancelled"));
            }
            n = k.needs().lease() => n,
        };

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "need_id": need.id,
                    "source": need.source,
                    "priority": format!("{:?}", need.priority).to_ascii_lowercase(),
                    "need": need.need,
                    "context": need.context,
                    "scope": need.scope,
                    "reply_to": need.reply_to.map(|u| u.to_string()),
                    "reconvene": need.reconvene,
                }),
            ))
            .await;
        Ok(())
    }
}

pub struct NeedFulfill;

impl NeedFulfill {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedFulfill {
    fn name(&self) -> &'static str {
        "need:fulfill"
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
        let need_id = data
            .get("need_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if need_id.is_empty() {
            return Err(KernelError::invalid_args("need_id is required"));
        }
        let existed = k.needs().fulfill(need_id).await.is_some();
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"fulfilled": existed})))
            .await;
        Ok(())
    }
}

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(NeedEnqueue::new()));
    dispatcher.register(Arc::new(NeedLease::new()));
    dispatcher.register(Arc::new(NeedFulfill::new()));
}
