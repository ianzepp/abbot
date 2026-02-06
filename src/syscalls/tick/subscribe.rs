use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct TickSubscribe;

impl TickSubscribe {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TickSubscribe {
    fn name(&self) -> &'static str {
        "tick:subscribe"
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
        let Some(tick) = k.tick() else {
            return Err(KernelError::internal("kernel tick not initialized"));
        };

        let mut rx = tick.receiver();
        // Emit the current tick immediately.
        {
            let cur = rx.borrow().clone();
            let _ = tx
                .send(Frame::event(
                    ctx.call_id,
                    json!({"kind": "SIGTICK", "seq": cur.seq, "now_ms": cur.now_ms, "dt_ms": cur.dt_ms}),
                ))
                .await;
        }

        loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => {
                    break;
                }
                changed = rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let cur = rx.borrow().clone();
                    if tx
                        .send(Frame::event(
                            ctx.call_id,
                            json!({"kind": "SIGTICK", "seq": cur.seq, "now_ms": cur.now_ms, "dt_ms": cur.dt_ms}),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }

        let _ = tx.send(Frame::done(ctx.call_id)).await;
        Ok(())
    }
}
