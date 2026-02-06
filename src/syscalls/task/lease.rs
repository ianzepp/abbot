use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct TaskLease;

impl TaskLease {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskLease {
    fn name(&self) -> &'static str {
        "task:lease"
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

        let hand_id = data
            .get("hand_id")
            .and_then(|v| v.as_str())
            .unwrap_or("hand")
            .trim();
        if hand_id.is_empty() {
            return Err(KernelError::invalid_args("hand_id is required"));
        }

        let task = tokio::select! {
            _ = ctx.cancel.cancelled() => {
                return Err(KernelError::cancelled("operation cancelled"));
            }
            t = k.tasks().lease(hand_id) => t,
        };

        k.bump_activity();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "task_id": task.id,
                    "head_id": task.head_id,
                    "scope": task.scope,
                    "goal": task.goal,
                    "input": task.input,
                    "notify_scope": task.notify_scope,
                    "reply_to": task.reply_to.map(|u| u.to_string()),
                }),
            ))
            .await;
        Ok(())
    }
}
