//! Sigcall Hub - Outbound frame broadcast and turn stream delivery
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! SigcallHub manages outbound frames from the kernel to clients. It serves
//! two roles:
//! - Broadcast: All sigcalls are broadcast to observers (monitors, TUI)
//! - Point-to-point: Turn stream frames are delivered to the specific client
//!   that opened a stream for (room, reply_to)
//!
//! The syscall refactor establishes turn streams as the canonical client-facing
//! output channel for chat:* syscalls. Sigcalls are the mechanism for emitting
//! frames onto those streams.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Dual delivery: Broadcast for observability, point-to-point for turn streams
//! - Room tagging: Frames are tagged with room in trace metadata for filtering
//! - Audit integration: All outbound frames are logged before delivery
//! - Replace semantics: open() replaces any existing stream to avoid stale senders

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;

use crate::kernel::Frame;
use crate::kernel::FrameStore;

use serde_json::json;

/// Turn stream identifier (room, reply_to).
///
/// WHY internal: Clients interact via open/send/close, not directly with keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReplyKey {
    room: String,
    thread_id: Uuid,
}

// =============================================================================
// SIGCALL HUB
// =============================================================================

/// Manages outbound sigcall frames (kernel -> client).
///
/// WHY this exists: Sigcalls are the output side of the syscall protocol.
/// chat:* syscalls emit frames via SigcallHub to deliver them to turn streams
/// and broadcast them to observers.
///
/// CONCURRENCY
/// -----------
/// Stream registration (open/close) locks the full stream map. Send acquires
/// a read lock to check for a stream, then delivers point-to-point. This is
/// acceptable because open/close are infrequent (one per turn segment) and
/// send is fast (async channel send, no I/O under lock).
pub struct SigcallHub {
    streams: Mutex<HashMap<ReplyKey, mpsc::Sender<Frame>>>,
    broadcast_tx: broadcast::Sender<Frame>,
    capacity: usize,
    frames: RwLock<Option<Arc<FrameStore>>>,
}

impl SigcallHub {
    pub fn new(broadcast_tx: broadcast::Sender<Frame>) -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
            broadcast_tx,
            capacity: 256,
            frames: RwLock::new(None),
        }
    }

    pub fn set_frames(&self, frames: Arc<FrameStore>) {
        if let Ok(mut f) = self.frames.write() {
            *f = Some(frames);
        }
    }

    pub fn with_capacity(mut self, capacity: usize) -> Self {
        if capacity > 0 {
            self.capacity = capacity;
        }
        self
    }

    /// Open a turn stream for (room, reply_to) and return receiver.
    ///
    /// WHY replace semantics: If a stream already exists for this key, it's
    /// replaced to avoid stale senders (e.g., head resumed after client
    /// reconnect). Old receiver will see channel close.
    pub async fn open(&self, room: &str, thread_id: Uuid) -> mpsc::Receiver<Frame> {
        let key = ReplyKey {
            room: room.to_string(),
            thread_id,
        };
        let (tx, rx) = mpsc::channel::<Frame>(self.capacity);
        let mut streams = self.streams.lock().await;
        streams.insert(key, tx);
        rx
    }

    /// Send a sigcall frame to turn stream and broadcast to observers.
    ///
    /// WHY dual delivery: Broadcast allows monitors/TUI to observe all sigcalls;
    /// point-to-point delivers turn stream frames to the specific client. Both
    /// are required for observability + correct turn stream semantics.
    ///
    /// WHY audit before delivery: Ensures frames are persisted even if client
    /// disconnects before receiving them.
    pub async fn send(&self, room: &str, thread_id: Uuid, frame: Frame) {
        // WHY tag room: Broadcast observers need room context for filtering
        // without parsing frame.data. Scope is metadata, not authorship.
        let frame = tag_frame_room(frame, room);

        // WHY audit outbound frames: Turn stream output must be logged for
        // replay, debugging, and compliance.
        let store = self.frames.read().ok().and_then(|f| f.as_ref().cloned());
        if let Some(store) = store {
            store.append(frame.clone()).await;
        }

        // WHY always broadcast: Observers (TUI, monitors) must see all sigcalls
        // regardless of whether a turn stream is open.
        let _ = self.broadcast_tx.send(frame.clone());

        // WHY point-to-point delivery: Turn stream frames must reach the specific
        // client for (room, reply_to). If no stream is open, frame is logged
        // but not delivered (client will query history).
        let key = ReplyKey {
            room: room.to_string(),
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

    /// Close a turn stream.
    ///
    /// WHY: chat:done and chat:error close the turn stream to signal the client
    /// that the segment is complete. Removing the sender causes the receiver to
    /// see channel close.
    pub async fn close(&self, room: &str, thread_id: Uuid) {
        let key = ReplyKey {
            room: room.to_string(),
            thread_id,
        };
        let mut streams = self.streams.lock().await;
        streams.remove(&key);
    }
}

/// Tag frame with room metadata for broadcast filtering.
///
/// WHY: Scope is turn context (main vs session/<hash>), not authorship. Storing
/// it in trace metadata keeps it separate from actor and avoids polluting data.
fn tag_frame_room(mut frame: Frame, room: &str) -> Frame {
    let room = room.trim();
    if room.is_empty() {
        return frame;
    }

    match frame.trace.take() {
        None => {
            frame.trace = Some(json!({"room": room}));
        }
        Some(mut t) => {
            if let Some(obj) = t.as_object_mut() {
                if !obj.contains_key("room") {
                    obj.insert("room".to_string(), json!(room));
                }
                frame.trace = Some(t);
            } else {
                frame.trace = Some(json!({"room": room, "trace": t}));
            }
        }
    }
    frame
}
