use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct StateQueryArgs {
    mode: String,
    #[serde(default)]
    #[allow(dead_code)]
    scope: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

pub struct StateQuery;

impl StateQuery {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for StateQuery {
    fn name(&self) -> &'static str {
        "state:query"
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

        let args: StateQueryArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let limit = args.limit.unwrap_or(20).clamp(1, 100);

        let result = match args.mode.as_str() {
            "messages" => {
                return Err(KernelError::invalid_args(
                    "introspect mode 'messages' removed (bus/store.db message history deprecated)",
                ));
            }
            "wants" => {
                let wants = store
                    .list_wants(limit)
                    .map_err(|e| KernelError::io(format!("query error: {e}")))?;
                let out: Vec<_> = wants
                    .iter()
                    .map(|w| {
                        json!({
                            "id": &w.id[..8.min(w.id.len())],
                            "want": w.want,
                            "priority": w.priority,
                            "source": w.source
                        })
                    })
                    .collect();
                json!({"wants": out, "count": out.len()})
            }
            "logs" => {
                let task_id = args
                    .task_id
                    .as_deref()
                    .ok_or_else(|| KernelError::invalid_args("task_id required for logs mode"))?;
                let execs = store
                    .get_hand_execs(task_id)
                    .map_err(|e| KernelError::io(format!("query error: {e}")))?;
                let out: Vec<_> = execs
                    .iter()
                    .map(|e| {
                        let output: String = e.output.chars().take(200).collect();
                        json!({
                            "step": e.step,
                            "tool": e.tool,
                            "success": e.success,
                            "output": output
                        })
                    })
                    .collect();
                json!({"logs": out, "count": out.len()})
            }
            "stats" => {
                json!({
                    "wants_pool": store.count_wants().unwrap_or(0),
                    "note": "recent message stats removed (store.db message history deprecated)"
                })
            }
            "needs" => {
                return Err(KernelError::invalid_args(
                    "introspect mode 'needs' removed (store.db message history deprecated)",
                ));
            }
            "tasks" | "goals" => {
                return Err(KernelError::invalid_args(
                    "introspect mode 'tasks' removed (store.db message history deprecated)",
                ));
            }
            _ => {
                return Err(KernelError::invalid_args(format!(
                    "unknown introspect mode: {}",
                    args.mode
                )));
            }
        };

        let _ = tx.send(Frame::ok(ctx.call_id, result)).await;
        Ok(())
    }
}
