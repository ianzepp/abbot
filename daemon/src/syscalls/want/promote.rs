//! Want:Promote - Convert want to executable need via EMS
//!
//! Promotes a want from the `wants` table to a need. The want is soft-updated
//! to status "promoted" (not hard-deleted), and a `need:enqueue` syscall is
//! dispatched with the want's data.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::ems::schema::rank_to_priority;
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Debug, Deserialize)]
struct WantPromoteArgs {
    id: String,
    #[serde(default)]
    priority: Option<String>,
}

pub struct WantPromote;

impl Default for WantPromote {
    fn default() -> Self {
        Self::new()
    }
}

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
        ctx.require_mutation()?;

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
        };

        let args: WantPromoteArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // Fetch the want from EMS
        let want_row = {
            let ems = ems.lock().await;
            let rows = ems
                .select(
                    "wants",
                    Some(&json!({"id": args.id, "status": "pending"})),
                    None,
                    None,
                    Some(1),
                    None,
                )
                .await
                .map_err(|e| KernelError::io(format!("failed to get want: {e}")))?;
            rows.into_iter().next()
        };

        let Some(want) = want_row else {
            let _ = tx
                .send(Frame::ok(
                    ctx.call_id,
                    json!({"promoted": false, "reason": "want not found"}),
                ))
                .await;
            return Ok(());
        };

        let want_text = want.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        let want_context = want.get("context").and_then(|v| v.as_str()).unwrap_or("");
        let want_priority_rank = want.get("priority").and_then(|v| v.as_i64()).unwrap_or(2);
        let want_priority = rank_to_priority(want_priority_rank);
        let priority_str = args.priority.as_deref().unwrap_or(want_priority);
        let need_id = Uuid::new_v4().to_string();

        // Soft-update want to "promoted" status
        {
            let mut ems = ems.lock().await;
            ems.update(
                "wants",
                &json!({"id": args.id}),
                &json!({
                    "status": "promoted",
                    "promoted_need_id": need_id,
                    "updated_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
                }),
            )
            .await
            .map_err(|e| KernelError::io(format!("failed to update want: {e}")))?;
        }

        // Dispatch need:enqueue
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "need:enqueue",
            json!({
                "need_id": need_id,
                "source": "mind",
                "priority": priority_str,
                "need": want_text,
                "context": want_context,
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
