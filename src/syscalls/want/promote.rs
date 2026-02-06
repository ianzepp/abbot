use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct WantPromoteArgs {
    id: String,
    #[serde(default)]
    priority: Option<String>,
}

pub struct WantPromote;

impl WantPromote {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for WantPromote {
    fn name(&self) -> &'static str {
        "want:promote"
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

        let args: WantPromoteArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let want = match store.get_want(&args.id) {
            Ok(Some(w)) => w,
            Ok(None) => {
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({"promoted": false, "reason": "want not found"}),
                    ))
                    .await;
                return Ok(());
            }
            Err(e) => return Err(KernelError::io(format!("failed to get want: {e}"))),
        };

        store
            .remove_want(&args.id)
            .map_err(|e| KernelError::io(format!("failed to remove want: {e}")))?;

        let priority_str = args.priority.as_deref().unwrap_or(&want.priority);
        let need_id = Uuid::new_v4().to_string();

        // Dispatch need:enqueue through the kernel
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "need:enqueue",
            json!({
                "need_id": need_id,
                "source": "mind",
                "priority": priority_str,
                "need": want.want,
                "context": want.context,
                "scope": "main",
                "reconvene": priority_str == "urgent",
            }),
        )
        .with_actor("system/mind".to_string());

        let mut rx = dispatcher.dispatch(
            req,
            ctx.cwd.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "promoted": true,
                    "want_id": args.id,
                    "need_id": need_id,
                    "priority": priority_str
                }),
            ))
            .await;

        Ok(())
    }
}
