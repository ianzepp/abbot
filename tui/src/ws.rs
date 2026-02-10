//! WebSocket client — connects to daemon, sends/receives raw Frames.

use std::collections::{HashSet, VecDeque};

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

// =============================================================================
// FRAME (local mirror of daemon's Frame, with Serialize for outbound)
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameOp {
    Req,
    Cancel,
    Ok,
    Error,
    Done,
    Item,
    Bytes,
    Event,
    Progress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub id: Uuid,
    #[serde(default)]
    pub ts: i64,
    pub op: FrameOp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl Frame {
    pub fn req(name: &str, data: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            ts: 0,
            op: FrameOp::Req,
            name: Some(name.to_string()),
            parent_id: None,
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(data),
        }
    }

    pub fn with_actor(mut self, actor: &str) -> Self {
        self.actor = Some(actor.to_string());
        self
    }
}

// =============================================================================
// WIRE TYPES (must match daemon/src/server/websocket.rs)
// =============================================================================

#[derive(Serialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
pub enum WsInMessage {
    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "frame")]
    FrameMsg { frame: Frame },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WsOutMessage {
    #[serde(rename = "connected")]
    Connected {
        #[allow(dead_code)]
        version: String,
    },

    #[serde(rename = "pong")]
    Pong {
        #[allow(dead_code)]
        timestamp_ms: i64,
    },

    #[serde(rename = "frame")]
    Frame(Frame),

    #[serde(rename = "error")]
    Error {
        #[allow(dead_code)]
        message: String,
    },
}

// =============================================================================
// EVENTS (background task → main loop)
// =============================================================================

pub enum WsEvent {
    Connected,
    Disconnected,
    ChatAck {
        room: String,
    },
    ChatDelta {
        room: String,
        content: String,
        seq: Option<u64>,
    },
    ChatTool {
        room: String,
        name: String,
    },
    ChatDone {
        room: String,
    },
    ChatError {
        room: String,
        message: String,
    },
    ChatStatus {
        room: String,
        status: String,
        actor: Option<String>,
        tool: Option<String>,
        summary: Option<String>,
        content: Option<String>,
    },
    ChatMind {
        room: String,
        content: String,
    },
    Frame(Frame),
    HandStart {
        #[allow(dead_code)]
        room: String,
        actor: String,
        tool: Option<String>,
        summary: Option<String>,
    },
    HandEnd {
        #[allow(dead_code)]
        room: String,
        #[allow(dead_code)]
        actor: String,
    },
    ReplaySync {
        room: String,
        max_ts: i64,
    },
    ReplayUser {
        room: String,
        content: String,
        seq: u64,
    },
}

// =============================================================================
// FRAME → EVENT MAPPING
// =============================================================================

/// Extract room from a Frame: check data.room, then trace.room, default "main".
fn extract_room(frame: &Frame) -> String {
    if let Some(data) = &frame.data
        && let Some(r) = data.get("room").and_then(|v| v.as_str())
    {
        return r.to_string();
    }
    if let Some(trace) = &frame.trace
        && let Some(r) = trace.get("room").and_then(|v| v.as_str())
    {
        return r.to_string();
    }
    "main".to_string()
}

/// Map a raw Frame to a typed WsEvent for the TUI event loop.
pub(crate) fn map_ws_frame(frame: &Frame) -> Option<WsEvent> {
    let data = frame.data.as_ref();
    let room = extract_room(frame);

    match frame.op {
        FrameOp::Event => {
            let kind = data
                .and_then(|d| d.get("kind"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match kind {
                "chat.ack" => Some(WsEvent::ChatAck { room }),
                "chat:user" => {
                    let content = data
                        .and_then(|d| d.get("content"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if content.is_empty() {
                        return None;
                    }
                    Some(WsEvent::ReplayUser {
                        room,
                        content,
                        seq: 0,
                    })
                }
                "mind:thought" => {
                    let content = data
                        .and_then(|d| d.get("content"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if content.is_empty() {
                        return None;
                    }
                    Some(WsEvent::ChatMind { room, content })
                }
                "hand:start" => {
                    let actor = frame.actor.clone()?;
                    let tool = frame.name.clone();
                    let summary = data
                        .and_then(|d| {
                            d.get("summary")
                                .or_else(|| d.get("prompt"))
                                .or_else(|| d.get("command"))
                                .or_else(|| d.get("tool"))
                        })
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    Some(WsEvent::HandStart {
                        room,
                        actor,
                        tool,
                        summary,
                    })
                }
                "hand:end" => {
                    let actor = frame.actor.clone()?;
                    Some(WsEvent::HandEnd { room, actor })
                }
                _ => {
                    // mind:thought can also appear as name (not kind)
                    if frame.name.as_deref() == Some("mind:thought") {
                        let content = data
                            .and_then(|d| d.get("content"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if content.is_empty() {
                            return None;
                        }
                        return Some(WsEvent::ChatMind { room, content });
                    }
                    None
                }
            }
        }
        FrameOp::Item => {
            let data_type = data
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match data_type {
                "text_delta" => {
                    let content = data
                        .and_then(|d| d.get("content").or_else(|| d.get("text")))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if content.is_empty() {
                        return None;
                    }
                    Some(WsEvent::ChatDelta {
                        room,
                        content,
                        seq: None,
                    })
                }
                "tool_call" => {
                    let name = data
                        .and_then(|d| d.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    Some(WsEvent::ChatTool { room, name })
                }
                "status" => {
                    let status = data
                        .and_then(|d| d.get("status"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let actor = data
                        .and_then(|d| d.get("actor"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let tool = data
                        .and_then(|d| d.get("tool"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let summary = data
                        .and_then(|d| d.get("summary"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let content = data
                        .and_then(|d| d.get("content"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    Some(WsEvent::ChatStatus {
                        room,
                        status,
                        actor,
                        tool,
                        summary,
                        content,
                    })
                }
                "done" => Some(WsEvent::ChatDone { room }),
                _ => None,
            }
        }
        FrameOp::Done => {
            if frame.name.as_deref() == Some("chat:message") {
                Some(WsEvent::ChatDone { room })
            } else {
                None
            }
        }
        FrameOp::Error => {
            let message = data
                .and_then(|d| d.get("message").or_else(|| d.get("error")))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error")
                .to_string();
            Some(WsEvent::ChatError { room, message })
        }
        _ => None,
    }
}

// =============================================================================
// BACKGROUND TASK
// =============================================================================

/// Max number of frame IDs to track for deduplication.
const DEDUP_CAP: usize = 4096;

pub async fn run_ws(
    addr: String,
    event_tx: mpsc::Sender<WsEvent>,
    mut cmd_rx: mpsc::Receiver<WsInMessage>,
) {
    loop {
        let url = format!("ws://{}/ws", addr);
        let ws = match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => ws,
            Err(_) => {
                let _ = event_tx.send(WsEvent::Disconnected).await;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };

        let _ = event_tx.send(WsEvent::Connected).await;
        let (mut sink, mut stream) = ws.split();

        // Bounded dedup set: VecDeque tracks insertion order for eviction.
        let mut seen_ids: HashSet<Uuid> = HashSet::with_capacity(DEDUP_CAP);
        let mut seen_order: VecDeque<Uuid> = VecDeque::with_capacity(DEDUP_CAP);

        loop {
            tokio::select! {
                msg = stream.next() => {
                    let Some(msg) = msg else { break };
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(out) = serde_json::from_str::<WsOutMessage>(&text) {
                                match out {
                                    WsOutMessage::Connected { .. } | WsOutMessage::Pong { .. } => {}
                                    WsOutMessage::Frame(frame) => {
                                        // Dedup by frame ID
                                        if !seen_ids.insert(frame.id) {
                                            continue;
                                        }
                                        seen_order.push_back(frame.id);
                                        if seen_order.len() > DEDUP_CAP
                                            && let Some(old) = seen_order.pop_front()
                                        {
                                            seen_ids.remove(&old);
                                        }

                                        // Map to typed event for chat/hand state
                                        if let Some(event) = map_ws_frame(&frame) {
                                            let _ = event_tx.send(event).await;
                                        }
                                        // Always forward raw Frame for hand_log + frames view
                                        let _ = event_tx.send(WsEvent::Frame(frame)).await;
                                    }
                                    WsOutMessage::Error { .. } => {}
                                }
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Err(_) => break,
                        _ => {}
                    }
                }

                cmd = cmd_rx.recv() => {
                    let Some(cmd) = cmd else { return };
                    if let Ok(json) = serde_json::to_string(&cmd)
                        && sink.send(Message::Text(json.into())).await.is_err()
                    {
                        break;
                    }
                }
            }
        }

        let _ = event_tx.send(WsEvent::Disconnected).await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
