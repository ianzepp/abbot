//! Need:Enqueue - Add work items to the EMS-backed priority queue
//!
//! Validates args, inserts a row into the EMS `needs` table with integer priority
//! for SQL-sortable ordering, then wakes one waiting leaser via Notify.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::ems::schema::priority_to_rank;
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct NeedEnqueue;

impl Default for NeedEnqueue {
    fn default() -> Self {
        Self::new()
    }
}

impl NeedEnqueue {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedEnqueue {
    fn name(&self) -> &'static str {
        "need:enqueue"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let actor = ctx.actor_str();
        let can_enqueue = actor == "user"
            || actor.starts_with("human/")
            || actor.starts_with("head/")
            || actor.starts_with("mind/")
            || actor.starts_with("system/");
        if !can_enqueue {
            return Err(KernelError::forbidden(
                "need enqueue requires trusted actor",
            ));
        }

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
        };

        // Validate required fields (auto-generate need_id if missing)
        let need_id = data
            .get("need_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let need_id = if need_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            need_id
        };

        let need = data
            .get("need")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if need.is_empty() {
            return Err(KernelError::invalid_args("need is required"));
        }

        let source = data
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .trim()
            .to_string();

        let room = data
            .get("room")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim()
            .to_string();

        let context = data
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let priority = data
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("normal")
            .to_string();

        let reply_to = data
            .get("reply_to")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let reconvene = data
            .get("reconvene")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let priority_rank = priority_to_rank(&priority);

        let row = json!({
            "id": need_id,
            "status": "pending",
            "priority": priority_rank,
            "prompt": need,
            "room": room,
            "priority_label": priority,
            "actor": source,
            "context": context,
            "reply_to": reply_to,
            "reconvene": if reconvene { "true" } else { "false" },
        });

        {
            let mut ems = ems.lock().await;
            ems.insert("needs", &row)
                .await
                .map_err(|e| KernelError::io(format!("failed to enqueue need: {e}")))?;
        }

        // Wake one waiting leaser
        k.needs().notify_enqueue();

        k.bump_activity();

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"enqueued": true})))
            .await;
        Ok(())
    }
}
