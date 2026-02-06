//! Task:Lease - Claim tasks from EMS with approximate round-robin fairness
//!
//! Uses `claim_one` to atomically find and update the next pending task to
//! "running" status. Blocks via Notify when no tasks are available.
//! Approximate round-robin: tries scope > last_leased_scope first, wraps around.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct TaskLease;

impl TaskLease {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskLease {
    fn name(&self) -> &'static str {
        "task:lease"
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

        let hand_id = data
            .get("hand_id")
            .and_then(|v| v.as_str())
            .unwrap_or("hand")
            .trim();
        if hand_id.is_empty() {
            return Err(KernelError::invalid_args("hand_id is required"));
        }

        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

        loop {
            // Try to claim one pending task with round-robin
            let last_scope = k.tasks().last_leased_scope().await;

            let claimed = {
                let mut ems_guard = ems.lock().unwrap();

                // Try scope > last_leased_scope first (approximate round-robin)
                let mut result = None;
                if let Some(ref ls) = last_scope {
                    result = ems_guard.claim_one(
                        "tasks",
                        &json!({"status": "pending", "scope": {"$gt": ls}}),
                        "\"scope\" ASC, \"created_at\" ASC",
                        &json!({
                            "status": "running",
                            "lease_owner": hand_id,
                            "leased_at": now,
                            "started_at": now,
                            "updated_at": now,
                        }),
                    ).map_err(|e| KernelError::io(format!("failed to lease task: {e}")))?;
                }

                // Wraparound: try any pending task
                if result.is_none() {
                    result = ems_guard.claim_one(
                        "tasks",
                        &json!({"status": "pending"}),
                        "\"scope\" ASC, \"created_at\" ASC",
                        &json!({
                            "status": "running",
                            "lease_owner": hand_id,
                            "leased_at": now,
                            "started_at": now,
                            "updated_at": now,
                        }),
                    ).map_err(|e| KernelError::io(format!("failed to lease task: {e}")))?;
                }

                result
            };

            if let Some(row) = claimed {
                let scope = row.get("scope").and_then(|v| v.as_str()).unwrap_or("main");
                k.tasks().set_last_leased_scope(scope).await;
                k.bump_activity();

                // Notify watcher for this task (status changed to running)
                let task_id = row.get("id").and_then(|v| v.as_str()).unwrap_or("");
                k.tasks().notify_watcher(task_id).await;

                // Parse batch_calls from JSON text
                let batch_calls = row
                    .get("batch_calls")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| v.as_array().cloned());

                let mut payload = json!({
                    "task_id": task_id,
                    "head_id": row.get("head_id").and_then(|v| v.as_str()).unwrap_or("unknown"),
                    "scope": scope,
                    "prompt": row.get("prompt").and_then(|v| v.as_str()).unwrap_or(""),
                    "input": row.get("input").and_then(|v| v.as_str()).unwrap_or(""),
                    "notify_scope": row.get("notify_scope").and_then(|v| v.as_str()),
                    "reply_to": row.get("reply_to").and_then(|v| v.as_str()),
                });

                if let Some(calls) = batch_calls {
                    payload["calls"] = json!(calls);
                }

                let _ = tx.send(Frame::ok(ctx.call_id, payload)).await;
                return Ok(());
            }

            // No task available — wait for notification
            tokio::select! {
                _ = ctx.cancel.cancelled() => {
                    return Err(KernelError::cancelled("operation cancelled"));
                }
                _ = k.tasks().wait_for_task() => {
                    // Retry the claim
                }
            }
        }
    }
}
