use std::collections::{BinaryHeap, HashMap};
use std::cmp::Ordering;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NeedPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Urgent = 3,
}

#[derive(Debug, Clone)]
pub struct NeedItem {
    pub id: String,
    pub source: String,
    pub priority: NeedPriority,
    pub need: String,
    pub context: String,
    pub scope: String,
    pub reply_to: Option<Uuid>,
    pub reconvene: bool,
    pub created_at: Instant,
}

impl Eq for NeedItem {}

impl PartialEq for NeedItem {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl PartialOrd for NeedItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NeedItem {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.priority.cmp(&other.priority) {
            Ordering::Equal => other.created_at.cmp(&self.created_at),
            other_ord => other_ord,
        }
    }
}

#[derive(Debug, Default)]
pub struct NeedKernel {
    queue: Mutex<BinaryHeap<NeedItem>>,
    active: Mutex<HashMap<String, NeedItem>>,
    notify: Notify,
}

impl NeedKernel {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn enqueue(&self, need: NeedItem) {
        {
            let mut q = self.queue.lock().await;
            q.push(need);
        }
        self.notify.notify_one();
    }

    pub async fn lease(&self) -> NeedItem {
        loop {
            if let Some(n) = {
                let mut q = self.queue.lock().await;
                q.pop()
            } {
                {
                    let mut active = self.active.lock().await;
                    active.insert(n.id.clone(), n.clone());
                }
                return n;
            }
            self.notify.notified().await;
        }
    }

    pub async fn fulfill(&self, need_id: &str) -> Option<NeedItem> {
        let mut active = self.active.lock().await;
        active.remove(need_id)
    }

    pub async fn counts(&self) -> (usize, usize) {
        let queued = self.queue.lock().await.len();
        let active = self.active.lock().await.len();
        (queued, active)
    }

    pub fn need_from_json(data: Value) -> Result<NeedItem, String> {
        let id = data
            .get("need_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() {
            return Err("need_id is required".to_string());
        }

        let need = data
            .get("need")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if need.is_empty() {
            return Err("need is required".to_string());
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
            .and_then(|s| match s {
                "low" => Some(NeedPriority::Low),
                "normal" => Some(NeedPriority::Normal),
                "high" => Some(NeedPriority::High),
                "urgent" => Some(NeedPriority::Urgent),
                _ => None,
            })
            .unwrap_or(NeedPriority::Normal);

        let reply_to = data
            .get("reply_to")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        let reconvene = data
            .get("reconvene")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(NeedItem {
            id,
            source,
            priority,
            need,
            context,
            scope,
            reply_to,
            reconvene,
            created_at: Instant::now(),
        })
    }
}
