use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;

use crate::kernel::AuditLog;
use crate::kernel::Frame;

use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReplyKey {
    scope: String,
    thread_id: Uuid,
}

/// Manages outbound sigcall frames (kernel -> client).
///
/// Sigcalls are broadcast to all observers AND optionally delivered
/// point-to-point to a specific client that opened a reply stream.
pub struct SigcallHub {
    streams: Mutex<HashMap<ReplyKey, mpsc::Sender<Frame>>>,
    broadcast_tx: broadcast::Sender<Frame>,
    capacity: usize,
    audit: RwLock<Option<Arc<AuditLog>>>,
}

impl SigcallHub {
    pub fn new(broadcast_tx: broadcast::Sender<Frame>) -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
            broadcast_tx,
            capacity: 256,
            audit: RwLock::new(None),
        }
    }

    pub fn set_audit(&self, audit: Arc<AuditLog>) {
        if let Ok(mut a) = self.audit.write() {
            *a = Some(audit);
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
        // Sigcalls are scoped to a session/thread; tag frames for broadcast observers without
        // overloading `actor` (authorship).
        let frame = tag_frame_scope(frame, scope);

        // Persist outbound frames when audit is enabled.
        let audit = self.audit.read().ok().and_then(|a| a.as_ref().cloned());
        if let Some(audit) = audit {
            audit.append(frame.clone()).await;
        }

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

fn tag_frame_scope(mut frame: Frame, scope: &str) -> Frame {
    let scope = scope.trim();
    if scope.is_empty() {
        return frame;
    }

    match frame.trace.take() {
        None => {
            frame.trace = Some(json!({"scope": scope}));
        }
        Some(mut t) => {
            if let Some(obj) = t.as_object_mut() {
                if !obj.contains_key("scope") {
                    obj.insert("scope".to_string(), json!(scope));
                }
                frame.trace = Some(t);
            } else {
                frame.trace = Some(json!({"scope": scope, "trace": t}));
            }
        }
    }
    frame
}
