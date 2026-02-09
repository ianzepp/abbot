//! Need:Lease - Claim next highest-priority need from EMS
//!
//! Uses `claim_one` to atomically find and update the next pending need to
//! "running" status. Blocks via Notify when no needs are available.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::ems::schema::rank_to_priority;
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct NeedLease;

impl Default for NeedLease {
    fn default() -> Self {
        Self::new()
    }
}

impl NeedLease {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for NeedLease {
    fn name(&self) -> &'static str {
        "need:lease"
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

        // Build where clause: always filter for pending status, merge optional caller filter
        let mut where_clause = json!({"status": "pending"});
        if let Some(filter) = data.get("filter").and_then(|v| v.as_object())
            && let Some(obj) = where_clause.as_object_mut()
        {
            for (k, v) in filter {
                obj.insert(k.clone(), v.clone());
            }
        }

        let now = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();

        loop {
            // Register interest BEFORE checking the condition to avoid lost wakeups.
            // If notify_one() fires between claim_one() returning None and our await,
            // we'll still see the notification because we registered first.
            let notified = k.needs().notified();
            tokio::pin!(notified);

            // Try to claim one pending need
            let claimed = {
                let mut ems = ems.lock().await;
                ems.claim_one(
                    "needs",
                    &where_clause,
                    "\"priority\" ASC, \"created_at\" ASC",
                    &json!({
                        "status": "running",
                        "updated_at": now,
                    }),
                )
                .await
                .map_err(|e| KernelError::io(format!("failed to lease need: {e}")))?
            };

            if let Some(row) = claimed {
                k.bump_activity();

                let need_id = row.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let actor = row
                    .get("actor")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let priority_rank = row.get("priority").and_then(|v| v.as_i64()).unwrap_or(2);
                let priority = rank_to_priority(priority_rank);
                let instruction = row.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                let context = row.get("context").and_then(|v| v.as_str()).unwrap_or("");
                let room = row.get("room").and_then(|v| v.as_str()).unwrap_or("main");
                let reply_to = row.get("reply_to").and_then(|v| v.as_str());
                let reconvene = row
                    .get("reconvene")
                    .and_then(|v| v.as_str())
                    .unwrap_or("false")
                    == "true";

                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({
                            "need_id": need_id,
                            "source": actor,
                            "priority": priority,
                            "need": instruction,
                            "context": context,
                            "room": room,
                            "reply_to": reply_to,
                            "reconvene": reconvene,
                        }),
                    ))
                    .await;
                return Ok(());
            }

            // No need available — wait for notification or cancellation
            tokio::select! {
                _ = ctx.cancel.cancelled() => {
                    return Err(KernelError::cancelled("operation cancelled"));
                }
                _ = &mut notified => {
                    // Retry the claim
                }
            }
        }
    }
}
