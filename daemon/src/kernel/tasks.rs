use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct BatchCall {
    pub name: String,
    pub args: Value,
}

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

#[derive(Debug, Clone)]
pub enum TaskStatus {
    Queued,
    Running {
        hand_id: String,
        started_at: Instant,
    },
    Done {
        ok: bool,
        summary: String,
        finished_at: Instant,
    },
}

#[derive(Debug, Default)]
pub struct TaskKernel {
    queues: Mutex<HashMap<String, VecDeque<TaskItem>>>,
    rr_scopes: Mutex<VecDeque<String>>,
    active: Mutex<HashMap<String, TaskStatus>>,
    watchers: Mutex<HashMap<String, Arc<Notify>>>,
    notify: Notify,
}

impl TaskKernel {
    pub fn new() -> Self {
        Self::default()
    }

    fn watcher_for_locked(
        watchers: &mut HashMap<String, Arc<Notify>>,
        task_id: &str,
    ) -> Arc<Notify> {
        watchers
            .entry(task_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    pub async fn watcher(&self, task_id: &str) -> Arc<Notify> {
        let mut watchers = self.watchers.lock().await;
        Self::watcher_for_locked(&mut watchers, task_id)
    }

    pub async fn status(&self, task_id: &str) -> Option<TaskStatus> {
        let active = self.active.lock().await;
        active.get(task_id).cloned()
    }

    pub async fn enqueue(&self, task: TaskItem) {
        {
            let mut queues = self.queues.lock().await;
            let mut rr = self.rr_scopes.lock().await;
            let q = queues
                .entry(task.scope.clone())
                .or_insert_with(VecDeque::new);
            let was_empty = q.is_empty();
            q.push_back(task.clone());
            if was_empty {
                rr.push_back(task.scope.clone());
            }
        }

        {
            let mut active = self.active.lock().await;
            active.insert(task.id.clone(), TaskStatus::Queued);
        }

        let n = {
            let mut watchers = self.watchers.lock().await;
            Self::watcher_for_locked(&mut watchers, &task.id)
        };
        n.notify_waiters();
        self.notify.notify_one();
    }

    pub async fn lease(&self, hand_id: &str) -> TaskItem {
        loop {
            let picked = {
                let mut queues = self.queues.lock().await;
                let mut rr = self.rr_scopes.lock().await;

                let mut picked: Option<TaskItem> = None;
                let mut tries = rr.len();
                while tries > 0 {
                    tries -= 1;
                    let Some(scope) = rr.pop_front() else {
                        break;
                    };
                    let q = queues.get_mut(&scope);
                    let Some(q) = q else {
                        continue;
                    };
                    if let Some(task) = q.pop_front() {
                        if !q.is_empty() {
                            rr.push_back(scope);
                        }
                        picked = Some(task);
                        break;
                    }
                }
                picked
            };

            if let Some(task) = picked {
                {
                    let mut active = self.active.lock().await;
                    active.insert(
                        task.id.clone(),
                        TaskStatus::Running {
                            hand_id: hand_id.to_string(),
                            started_at: Instant::now(),
                        },
                    );
                }

                let n = {
                    let mut watchers = self.watchers.lock().await;
                    Self::watcher_for_locked(&mut watchers, &task.id)
                };
                n.notify_waiters();
                return task;
            }

            self.notify.notified().await;
        }
    }

    pub async fn complete(&self, task_id: &str, ok: bool, summary: String) {
        {
            let mut active = self.active.lock().await;
            active.insert(
                task_id.to_string(),
                TaskStatus::Done {
                    ok,
                    summary,
                    finished_at: Instant::now(),
                },
            );
        }
        let n = {
            let mut watchers = self.watchers.lock().await;
            Self::watcher_for_locked(&mut watchers, task_id)
        };
        n.notify_waiters();
    }

    pub async fn counts(&self) -> (usize, usize, usize) {
        let queued = {
            let queues = self.queues.lock().await;
            queues.values().map(|q| q.len()).sum()
        };
        let (running, done) = {
            let active = self.active.lock().await;
            let mut running = 0;
            let mut done = 0;
            for v in active.values() {
                match v {
                    TaskStatus::Queued => {}
                    TaskStatus::Running { .. } => running += 1,
                    TaskStatus::Done { .. } => done += 1,
                }
            }
            (running, done)
        };
        (queued, running, done)
    }

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
                    let args = item.get("args").cloned().unwrap_or(Value::Object(Default::default()));
                    Some(BatchCall { name, args })
                })
                .collect()
        });

        Ok(TaskItem {
            id,
            head_id,
            scope,
            prompt: prompt.clone(),
            input: if input.trim().is_empty() { prompt } else { input },
            notify_scope,
            reply_to,
            created_at: Instant::now(),
            batch_calls,
        })
    }
}
