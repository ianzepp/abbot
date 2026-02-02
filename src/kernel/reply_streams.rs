use std::collections::HashMap;

use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

use crate::kernel::Frame;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReplyKey {
    scope: String,
    thread_id: Uuid,
}

#[derive(Debug, Default)]
pub struct ReplyStreamManager {
    streams: Mutex<HashMap<ReplyKey, mpsc::Sender<Frame>>>,
    capacity: usize,
}

impl ReplyStreamManager {
    pub fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
            capacity: 256,
        }
    }

    pub fn with_capacity(mut self, capacity: usize) -> Self {
        if capacity > 0 {
            self.capacity = capacity;
        }
        self
    }

    pub async fn open(&self, scope: &str, thread_id: Uuid) -> mpsc::Receiver<Frame> {
        let key = ReplyKey {
            scope: scope.to_string(),
            thread_id,
        };
        let (tx, rx) = mpsc::channel::<Frame>(self.capacity);
        let mut streams = self.streams.lock().await;
        // Replace any existing stream for this (scope, thread) to avoid stale senders.
        streams.insert(key, tx);
        rx
    }

    pub async fn send(&self, scope: &str, thread_id: Uuid, frame: Frame) -> Result<(), ()> {
        let key = ReplyKey {
            scope: scope.to_string(),
            thread_id,
        };
        let tx = {
            let streams = self.streams.lock().await;
            streams.get(&key).cloned()
        };
        let Some(tx) = tx else {
            return Err(());
        };
        tx.send(frame).await.map_err(|_| ())
    }

    pub async fn close(&self, scope: &str, thread_id: Uuid) {
        let key = ReplyKey {
            scope: scope.to_string(),
            thread_id,
        };
        let mut streams = self.streams.lock().await;
        streams.remove(&key);
    }
}
