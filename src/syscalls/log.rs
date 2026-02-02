use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, LoggedFrame, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct LogAppend;

impl LogAppend {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LogAppend {
    fn name(&self) -> &'static str {
        "log:append"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // NOTE: Logging is intentionally best-effort.
        // This syscall emits a FrameOp::Event which is then picked up by the kernel audit
        // logger asynchronously. The Ok() response does NOT guarantee the log entry has
        // been durably committed to SQLite at the time it is returned.

        let kind = data
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("log")
            .trim();
        if kind.is_empty() {
            return Err(KernelError::invalid_args("kind is required"));
        }

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        let payload = serde_json::json!({
            "kind": kind,
            "scope": scope,
            "data": data.get("data").cloned().unwrap_or(serde_json::Value::Null),
        });

        let _ = tx.send(Frame::event(ctx.call_id, payload)).await;
        let _ = tx.send(Frame::ok(ctx.call_id, serde_json::json!({"logged": true}))).await;
        Ok(())
    }
}

pub struct LogTail;

impl LogTail {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LogTail {
    fn name(&self) -> &'static str {
        "log:tail"
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
        let audit = k
            .audit()
            .ok_or_else(|| KernelError::internal("audit log not initialized"))?;

        let mut after_seq = data
            .get("since")
            .or_else(|| data.get("after_seq"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let limit = data
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(200)
            .clamp(1, 2000) as usize;

        loop {
            ctx.check_cancelled()?;

            let batch: Vec<LoggedFrame> = audit
                .read_since(after_seq, limit)
                .map_err(|e| KernelError::io(format!("log read failed: {e}")))?;

            if !batch.is_empty() {
                for item in &batch {
                    let _ = tx
                        .send(Frame::item(
                            ctx.call_id,
                            json!({
                                "seq": item.seq,
                                "ts_ms": item.ts_ms,
                                "frame": item.frame,
                            }),
                        ))
                        .await;
                }
                after_seq = batch.last().map(|x| x.seq).unwrap_or(after_seq);
                continue;
            }

            audit.wait_for_seq(after_seq).await;
        }
    }
}

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(LogAppend::new()));
    dispatcher.register(Arc::new(LogTail::new()));
}
