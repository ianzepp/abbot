use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct StmUpdateArgs {
    op: String,
    #[serde(default)]
    content: String,
}

pub struct StmUpdate;

impl StmUpdate {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for StmUpdate {
    fn name(&self) -> &'static str {
        "stm:update"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;

        let head_id = super::read::extract_head_id(ctx)?;

        let args: StmUpdateArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let path = crate::runtime::workspace_head_memory(&ctx.cwd, &head_id);
        let current = match crate::runtime::read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
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

        let new_stm = match args.op.as_str() {
            "set" => args.content.clone(),
            "append" => {
                if current.is_empty() {
                    args.content.clone()
                } else if args.content.is_empty() {
                    current
                } else {
                    format!("{}\n\n{}", current, args.content)
                }
            }
            "clear" => String::new(),
            _ => {
                return Err(KernelError::invalid_args(format!(
                    "unknown op: {}",
                    args.op
                )))
            }
        };

        if let Err(e) = crate::runtime::atomic_write_file_0600(&path, &new_stm) {
            return Err(KernelError::io(format!("failed to save STM file: {e}")));
        }

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "head_id": head_id,
                    "op": args.op,
                    "stm_len": new_stm.len()
                }),
            ))
            .await;

        Ok(())
    }
}
