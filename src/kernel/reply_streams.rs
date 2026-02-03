use std::collections::HashMap;

use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;

use crate::kernel::Frame;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReplyKey {
    scope: String,
    thread_id: Uuid,
}

/// Manages outbound sigcall frames (kernel → client).
///
/// Sigcalls are broadcast to all observers AND optionally delivered
/// point-to-point to a specific client that opened a reply stream.
pub struct ReplyStreamManager {
    streams: Mutex<HashMap<ReplyKey, mpsc::Sender<Frame>>>,
    broadcast_tx: broadcast::Sender<Frame>,
    capacity: usize,
}

impl ReplyStreamManager {
    pub fn new(broadcast_tx: broadcast::Sender<Frame>) -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
            broadcast_tx,
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

    /// Send a sigcall frame.
    ///
    /// The frame is always broadcast to all observers. If a point-to-point
    /// reply stream is open for this (scope, thread_id), it's also delivered there.
    pub async fn send(&self, scope: &str, thread_id: Uuid, frame: Frame) {
        // Sigcalls are scoped to a session/thread; tag the frame so broadcast observers can
        // attribute it without out-of-band context.
        let frame = if frame.actor.is_some() {
            frame
        } else {
            frame.with_scope(scope)
        };

        // Always broadcast sigcalls
        let _ = self.broadcast_tx.send(frame.clone());

        // Also deliver point-to-point if a stream is open
        let key = ReplyKey {
            scope: scope.to_string(),
            thread_id,
        };
        let tx = {
            let streams = self.streams.lock().await;
            streams.get(&key).cloned()
        };
        if let Some(tx) = tx {
            let _ = tx.send(frame).await;
        }
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
