use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct LtmUpdateArgs {
    ops: Vec<LtmOp>,
}

#[derive(Debug, Deserialize)]
struct LtmOp {
    kind: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    pattern: String,
}

pub struct LtmUpdate;

impl LtmUpdate {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LtmUpdate {
    fn name(&self) -> &'static str {
        "ltm:update"
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

        let args: LtmUpdateArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let path = crate::runtime::workspace_mind_memory(&ctx.cwd);
        let current = match crate::runtime::read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // One-time migration from legacy DB location.
                let legacy = store.get_head_ltm("conclave").unwrap_or_default();
                if !legacy.trim().is_empty() {
                    let _ = crate::runtime::atomic_write_file_0600(&path, legacy.trim());
                    legacy
                } else {
                    String::new()
                }
            }
            Err(_) => String::new(),
        };

        let mut ltm = current.clone();
        let mut applied = Vec::new();

        for op in args.ops {
            match op.kind.as_str() {
                "append" => {
                    let content = op.content.trim();
                    if content.is_empty() {
                        continue;
                    }
                    if !ltm.is_empty() {
                        ltm.push_str("\n\n");
                    }
                    ltm.push_str(content);
                    applied.push(json!({"kind": "append"}));
                }
                "replace" => {
                    let pattern = op.pattern;
                    if pattern.is_empty() {
                        continue;
                    }
                    if let Some(pos) = ltm.find(&pattern) {
                        let end = pos + pattern.len();
                        let replacement = op.content;
                        ltm.replace_range(pos..end, &replacement);
                        applied.push(json!({"kind": "replace", "pattern": pattern}));
                    }
                }
                "remove" => {
                    let pattern = op.pattern;
                    if pattern.is_empty() {
                        continue;
                    }
                    if ltm.contains(&pattern) {
                        ltm = ltm.replace(&pattern, "");
                        while ltm.contains("\n\n\n") {
                            ltm = ltm.replace("\n\n\n", "\n\n");
                        }
                        ltm = ltm.trim().to_string();
                        applied.push(json!({"kind": "remove", "pattern": pattern}));
                    }
                }
                _ => {}
            }
        }

        if ltm != current {
            if let Err(e) = crate::runtime::atomic_write_file_0600(&path, &ltm) {
                return Err(KernelError::io(format!("failed to save LTM file: {e}")));
            }
        }

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"applied": applied, "ltm_len": ltm.len()}),
            ))
            .await;

        Ok(())
    }
}
