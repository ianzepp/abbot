use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

/// Lightweight DTO for batch tool calls — used by hand runtime to parse
/// batch_calls from task lease JSON.
#[derive(Debug, Clone)]
pub struct BatchCall {
    pub name: String,
    pub args: Value,
}

/// Lightweight DTO for task data — used by TaskKernel::task_from_json()
/// to validate enqueue arguments. Fields map to EMS `tasks` table columns.
#[derive(Debug, Clone)]
pub struct TaskItem {
    pub id: String,
    pub head_id: String,
    pub scope: String,
    pub prompt: String,
    pub input: String,
    pub notify_scope: Option<String>,
    pub reply_to: Option<Uuid>,
    pub created_at: Instant,
    pub batch_calls: Option<Vec<BatchCall>>,
}

/// Slimmed-down TaskKernel — only Notify for wakeup + per-task watchers.
/// All queue/status state now lives in EMS.
#[derive(Debug, Default)]
pub struct TaskKernel {
    notify: Notify,
    watchers: Mutex<HashMap<String, Arc<Notify>>>,
    last_leased_scope: Mutex<Option<String>>,
}

impl TaskKernel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn notify_enqueue(&self) {
        self.notify.notify_one();
    }

    pub async fn wait_for_task(&self) {
        self.notify.notified().await;
    }

    pub async fn last_leased_scope(&self) -> Option<String> {
        self.last_leased_scope.lock().await.clone()
    }

    pub async fn set_last_leased_scope(&self, scope: &str) {
        *self.last_leased_scope.lock().await = Some(scope.to_string());
    }

    pub async fn watcher(&self, task_id: &str) -> Arc<Notify> {
        let mut watchers = self.watchers.lock().await;
        watchers
            .entry(task_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    pub async fn notify_watcher(&self, task_id: &str) {
        let watchers = self.watchers.lock().await;
        if let Some(n) = watchers.get(task_id) {
            n.notify_waiters();
        }
    }

    /// Parse task fields from JSON — validation helper used by task:enqueue.
    pub fn task_from_json(data: Value) -> Result<TaskItem, String> {
        let prompt = data
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if prompt.is_empty() {
            return Err("prompt is required".to_string());
        }

        let id = data
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let head_id = data
            .get("head_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .trim()
            .to_string();

        let input = data
            .get("input")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim()
            .to_string();

        let notify_scope = data
            .get("notify_scope")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let reply_to = data
            .get("reply_to")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        let batch_calls = data.get("calls").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let name = item.get("name")?.as_str()?.to_string();
                    let args = item
                        .get("args")
                        .cloned()
                        .unwrap_or(Value::Object(Default::default()));
                    Some(BatchCall { name, args })
                })
                .collect()
        });

        Ok(TaskItem {
            id,
            head_id,
            scope,
            prompt: prompt.clone(),
            input: if input.trim().is_empty() {
                prompt
            } else {
                input
            },
            notify_scope,
            reply_to,
            created_at: Instant::now(),
            batch_calls,
        })
    }
}
