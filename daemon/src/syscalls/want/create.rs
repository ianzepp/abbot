//! Want:Create - Add new want/goal to EMS-backed queue
//!
//! Creates a new want in the EMS `wants` table. Wants represent aspirational
//! goals that may be promoted to executable "needs" later.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct WantCreateArgs {
    want: String,
    #[serde(default)]
    context: String,
    #[serde(default)]
    priority: Option<String>,
}

pub struct WantCreate;

impl WantCreate {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for WantCreate {
    fn name(&self) -> &'static str {
        "want:create"
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
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
        };

        let args: WantCreateArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.want.trim().is_empty() {
            return Err(KernelError::invalid_args("want is required"));
        }

        let priority = args.priority.as_deref().unwrap_or("normal");
        let want_id = Uuid::new_v4().to_string();

        let row = json!({
            "id": want_id,
            "status": "pending",
            "want": args.want,
            "context": args.context,
            "priority": priority,
            "source": "mind",
        });

        {
            let mut ems = ems.lock().await;
            ems.insert("wants", &row)
                .await
                .map_err(|e| KernelError::io(format!("failed to add want: {e}")))?;
        }

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "want_id": want_id,
                    "priority": priority,
                    "status": "added"
                }),
            ))
            .await;

        Ok(())
    }
}
