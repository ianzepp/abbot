use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct StmRead;

impl StmRead {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for StmRead {
    fn name(&self) -> &'static str {
        "stm:read"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let head_id = extract_head_id(ctx)?;

        let path = crate::runtime::workspace_head_memory(&ctx.cwd, &head_id);
        let stm = match crate::runtime::read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // One-time migration from legacy DB location.
                let Some(k) = Kernel::get() else {
                    return Err(KernelError::internal("kernel not initialized"));
                };
                let Some(store) = k.store() else {
                    return Err(KernelError::internal("kernel store not attached"));
                };
                let legacy = store.get_head_stm(&head_id).unwrap_or_default();
                if !legacy.trim().is_empty() {
                    let _ = crate::runtime::atomic_write_file_0600(&path, legacy.trim());
                    legacy
                } else {
                    String::new()
                }
            }
            Err(_) => String::new(),
        };

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "head_id": head_id,
                    "stm": stm,
                    "len": stm.len()
                }),
            ))
            .await;

        Ok(())
    }
}

pub(crate) fn extract_head_id(ctx: &SyscallContext) -> Result<String, KernelError> {
    let actor = ctx
        .actor
        .as_deref()
        .unwrap_or("");
    if let Some(id) = actor.strip_prefix("head/") {
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    Err(KernelError::invalid_args("actor must be head/<id>"))
}
