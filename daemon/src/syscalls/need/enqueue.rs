//! Need:Enqueue - Add work items to the EMS-backed priority queue
//!
//! Validates args, inserts a row into the EMS `needs` table with priority_rank
//! for SQL-sortable ordering, then wakes one waiting leaser via Notify.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

fn priority_to_rank(priority: &str) -> i64 {
    match priority {
        "urgent" => 0,
        "high" => 1,
        "normal" => 2,
        "low" => 3,
        _ => 2,
    }
}

pub struct NeedEnqueue;

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

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(ems) = k.ems() else {
            return Err(KernelError::internal("EMS not attached"));
        };

        // Validate required fields
        let need_id = data
            .get("need_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if need_id.is_empty() {
            return Err(KernelError::invalid_args("need_id is required"));
        }

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

        let scope = data
            .get("scope")
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
            "priority": priority,
            "priority_rank": priority_rank,
            "actor": source,
            "instruction": need,
            "context": context,
            "scope": scope,
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
