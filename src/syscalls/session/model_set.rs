use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct SessionModelSetArgs {
    model: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    reset: Option<bool>,
}

pub struct SessionModelSet;

impl SessionModelSet {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for SessionModelSet {
    fn name(&self) -> &'static str {
        "session:model_set"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        let args: SessionModelSetArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let model = args.model.trim();
        if model.is_empty() {
            return Err(KernelError::invalid_args("model is empty"));
        }

        let scope = args
            .scope
            .as_deref()
            .unwrap_or("main");

        store
            .set_session_model(scope, model)
            .map_err(|e| KernelError::io(format!("failed to set session model: {e}")))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "scope": scope,
                    "model": model,
                    "reset": args.reset.unwrap_or(false)
                }),
            ))
            .await;

        Ok(())
    }
}
