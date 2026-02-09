// Bidirectional WebSocket for web UI.
//
// Outbound: simplified frame broadcast, chat streaming (delta/tool/done/error), frame detail.
// Inbound: ping, chat.send, chat.cancel, frame.detail.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{
        ConnectInfo, State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::debug;
use uuid::Uuid;

use crate::history::Store;
use crate::kernel::{Frame, FrameOp, FrameStore};
use crate::runtime::Kernel;

// =============================================================================
// STATE
// =============================================================================

#[derive(Clone)]
pub struct WsState {
    pub store: Arc<Store>,
}

impl WsState {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

// =============================================================================
// WIRE FRAME (simplified for broadcast)
// =============================================================================

#[derive(Clone, Debug, Serialize)]
pub struct WireFrame {
    pub id: String,
    pub ts: i64,
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    pub summary: String,
}

fn simplify_frame(frame: &Frame) -> WireFrame {
    let room = frame
        .trace
        .as_ref()
        .and_then(|t| t.get("room"))
        .and_then(|s| s.as_str())
        .or_else(|| {
            frame
                .data
                .as_ref()
                .and_then(|d| d.get("room"))
                .and_then(|s| s.as_str())
        })
        .map(|s| s.to_string());

    let op_str = serde_json::to_value(&frame.op)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| format!("{:?}", frame.op).to_lowercase());

    WireFrame {
        id: frame.id.to_string(),
        ts: frame.ts,
        op: op_str,
        name: frame.name.clone(),
        parent_id: frame.parent_id.map(|u| u.to_string()),
        actor: frame.actor.clone(),
        room,
        summary: summarize_frame(frame),
    }
}

fn summarize_frame(frame: &Frame) -> String {
    let Some(data) = &frame.data else {
        return String::new();
    };

    // Chat frames: show content type + preview
    if let Some(typ) = data.get("type").and_then(|v| v.as_str()) {
        return match typ {
            "text_delta" => {
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let preview = truncate(content, 60);
                format!("text: {}", preview)
            }
            "tool_call" => {
                let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                format!("tool: {}", name)
            }
            "thinking" => "thinking...".into(),
            "done" => "done".into(),
            other => other.to_string(),
        };
    }

    // Needs/tasks
    if let Some(need) = data.get("need").and_then(|v| v.as_str()) {
        return truncate(need, 60).to_string();
    }
    if let Some(prompt) = data.get("prompt").and_then(|v| v.as_str()) {
        return truncate(prompt, 60).to_string();
    }

    // Errors
    if let Some(msg) = data.get("message").and_then(|v| v.as_str()) {
        return truncate(msg, 60).to_string();
    }

    // Content field
    if let Some(content) = data.get("content").and_then(|v| v.as_str()) {
        return truncate(content, 60).to_string();
    }

    // Kind field
    if let Some(kind) = data.get("kind").and_then(|v| v.as_str()) {
        return kind.to_string();
    }

    // Fallback: show top-level keys
    if let Some(obj) = data.as_object() {
        let keys: Vec<&str> = obj.keys().map(|k| k.as_str()).take(5).collect();
        return format!("{{{}}}", keys.join(", "));
    }

    String::new()
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let end = s.floor_char_boundary(max);
        &s[..end]
    }
}

// =============================================================================
// WIRE PROTOCOL
// =============================================================================

#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
enum WsOutMessage {
    #[serde(rename = "connected")]
    Connected { version: &'static str },

    #[serde(rename = "pong")]
    Pong { timestamp_ms: i64 },

    #[serde(rename = "frame")]
    Frame(WireFrame),

    #[serde(rename = "frame.detail")]
    FrameDetail {
        id: String,
        data: Option<Value>,
        trace: Option<Value>,
    },

    #[serde(rename = "chat.ack")]
    ChatAck {
        room: String,
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },

    #[serde(rename = "chat.delta")]
    ChatDelta {
        room: String,
        thread_id: String,
        content: String,
    },

    #[serde(rename = "chat.tool")]
    ChatTool {
        room: String,
        thread_id: String,
        tool_call_id: String,
        name: String,
        arguments: String,
    },

    #[serde(rename = "chat.done")]
    ChatDone {
        room: String,
        thread_id: String,
        reason: String,
    },

    #[serde(rename = "chat.error")]
    ChatError {
        room: String,
        thread_id: String,
        code: String,
        message: String,
    },

    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsInMessage {
    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "chat.send")]
    ChatSend {
        room: String,
        text: String,
        #[serde(default)]
        id: Option<String>,
    },

    #[serde(rename = "chat.cancel")]
    ChatCancel { room: String },

    #[serde(rename = "frame.detail")]
    FrameDetail { id: String },
}

// =============================================================================
// ACTIVE TURN TRACKING
// =============================================================================

struct ActiveTurn {
    thread_id: Uuid,
    reader_handle: JoinHandle<()>,
}

// =============================================================================
// HANDLER
// =============================================================================

pub async fn ws_handler(
    ConnectInfo(peer_addr): ConnectInfo<std::net::SocketAddr>,
    ws: WebSocketUpgrade,
    State(state): State<WsState>,
) -> Response {
    if !peer_addr.ip().is_loopback() {
        return (StatusCode::FORBIDDEN, "Abbot is loopback-only").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: WsState) {
    let (ws_sender, mut ws_receiver) = socket.split();

    // Writer task: mpsc → WebSocket
    let (out_tx, mut out_rx) = mpsc::channel::<WsOutMessage>(256);
    let writer_handle = tokio::spawn(async move {
        let mut ws_sender = ws_sender;
        while let Some(msg) = out_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg)
                && ws_sender.send(WsMessage::Text(json.into())).await.is_err()
            {
                break;
            }
        }
    });

    // Send connected
    let _ = out_tx
        .send(WsOutMessage::Connected { version: "0.1.0" })
        .await;

    let Some(k) = Kernel::get() else {
        let _ = out_tx
            .send(WsOutMessage::Error {
                message: "Kernel not initialized".into(),
            })
            .await;
        return;
    };

    // Send recent frames (simplified)
    if let Some(frames) = k.frames()
        && let Ok(recent) = frames.read_recent(100).await
    {
        debug!(
            count = recent.len(),
            "sending recent frames to new websocket client"
        );
        for stored in recent {
            let wire = simplify_frame(&stored.frame);
            let _ = out_tx.send(WsOutMessage::Frame(wire)).await;
        }
    }

    let mut frame_rx = k.subscribe_frames().await;
    let mut active_turns: HashMap<String, ActiveTurn> = HashMap::new();

    loop {
        tokio::select! {
            msg = ws_receiver.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Ok(WsMessage::Text(text)) => {
                        match serde_json::from_str::<WsInMessage>(&text) {
                            Ok(WsInMessage::Ping) => {
                                let _ = out_tx.send(WsOutMessage::Pong { timestamp_ms: now_ms() }).await;
                            }
                            Ok(WsInMessage::ChatSend { room, text, id }) => {
                                handle_chat_send(
                                    &state,
                                    &out_tx,
                                    &mut active_turns,
                                    room,
                                    text,
                                    id,
                                ).await;
                            }
                            Ok(WsInMessage::ChatCancel { room }) => {
                                handle_chat_cancel(&mut active_turns, &room).await;
                            }
                            Ok(WsInMessage::FrameDetail { id }) => {
                                handle_frame_detail(&out_tx, &id).await;
                            }
                            Err(_) => {}
                        }
                    }
                    Ok(WsMessage::Close(_)) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }

            frame = frame_rx.recv() => {
                match frame {
                    Ok(frame) => {
                        let wire = simplify_frame(&frame);
                        if out_tx.send(WsOutMessage::Frame(wire)).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        debug!(skipped = n, "websocket client lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        }
    }

    // Cancel all active turns on disconnect
    for (room, turn) in active_turns.drain() {
        turn.reader_handle.abort();
        dispatch_cancel(&room, turn.thread_id).await;
    }

    writer_handle.abort();
    debug!("websocket handler finished");
}

// =============================================================================
// CHAT SEND
// =============================================================================

async fn handle_chat_send(
    state: &WsState,
    out_tx: &mpsc::Sender<WsOutMessage>,
    active_turns: &mut HashMap<String, ActiveTurn>,
    room: String,
    text: String,
    client_id: Option<String>,
) {
    if text.trim().is_empty() {
        let _ = out_tx
            .send(WsOutMessage::Error {
                message: "Empty message".into(),
            })
            .await;
        return;
    }

    let Some(k) = Kernel::get() else {
        return;
    };

    // Cancel existing turn for this room if any
    if let Some(prev) = active_turns.remove(&room) {
        prev.reader_handle.abort();
        dispatch_cancel(&room, prev.thread_id).await;
    }

    let thread_id = Uuid::new_v4();

    // Open turn stream BEFORE dispatching to avoid race
    let rx = k.sigcalls().open(&room, thread_id).await;
    let _ = state.store.set_active_thread(&room, thread_id).await;

    // Ack
    let _ = out_tx
        .send(WsOutMessage::ChatAck {
            room: room.clone(),
            thread_id: thread_id.to_string(),
            client_id,
        })
        .await;

    // Dispatch chat:message syscall
    {
        let req = Frame::req(
            "chat:message",
            serde_json::json!({
                "room": &room,
                "reply_to": thread_id.to_string(),
                "content": &text,
                "interactive": true,
            }),
        )
        .with_actor("user");

        let dispatcher = k.dispatcher().await;
        let mut dispatch_rx = dispatcher.dispatch(
            req,
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = dispatch_rx.recv().await;
    }

    // Spawn reader task to convert turn stream frames → chat events
    let reader_out_tx = out_tx.clone();
    let reader_room = room.clone();
    let reader_thread_id = thread_id;
    let reader_handle = tokio::spawn(async move {
        turn_stream_reader(rx, reader_out_tx, reader_room, reader_thread_id).await;
    });

    active_turns.insert(
        room,
        ActiveTurn {
            thread_id,
            reader_handle,
        },
    );
}

async fn turn_stream_reader(
    mut rx: mpsc::Receiver<Frame>,
    out_tx: mpsc::Sender<WsOutMessage>,
    room: String,
    thread_id: Uuid,
) {
    let tid = thread_id.to_string();

    while let Some(frame) = rx.recv().await {
        let msg = match frame.op {
            FrameOp::Item => {
                let Some(data) = &frame.data else {
                    continue;
                };
                match data.get("type").and_then(|v| v.as_str()) {
                    Some("text_delta") => {
                        let content = data
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if content.is_empty() {
                            continue;
                        }
                        WsOutMessage::ChatDelta {
                            room: room.clone(),
                            thread_id: tid.clone(),
                            content,
                        }
                    }
                    Some("tool_call") => {
                        let tool_call_id = data
                            .get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = data
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = data
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        WsOutMessage::ChatTool {
                            room: room.clone(),
                            thread_id: tid.clone(),
                            tool_call_id,
                            name,
                            arguments,
                        }
                    }
                    Some("done") => WsOutMessage::ChatDone {
                        room: room.clone(),
                        thread_id: tid.clone(),
                        reason: "complete".into(),
                    },
                    _ => continue,
                }
            }
            FrameOp::Error => {
                let msg = frame
                    .data
                    .as_ref()
                    .and_then(|v| v.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                let code = frame
                    .data
                    .as_ref()
                    .and_then(|v| v.get("code"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("E_UNKNOWN")
                    .to_string();
                WsOutMessage::ChatError {
                    room: room.clone(),
                    thread_id: tid.clone(),
                    code,
                    message: msg,
                }
            }
            FrameOp::Done => WsOutMessage::ChatDone {
                room: room.clone(),
                thread_id: tid.clone(),
                reason: "complete".into(),
            },
            _ => continue,
        };

        let is_terminal = matches!(
            msg,
            WsOutMessage::ChatDone { .. } | WsOutMessage::ChatError { .. }
        );
        if out_tx.send(msg).await.is_err() {
            break;
        }
        if is_terminal {
            break;
        }
    }
}

// =============================================================================
// CHAT CANCEL
// =============================================================================

async fn handle_chat_cancel(active_turns: &mut HashMap<String, ActiveTurn>, room: &str) {
    if let Some(turn) = active_turns.remove(room) {
        turn.reader_handle.abort();
        dispatch_cancel(room, turn.thread_id).await;
    }
}

async fn dispatch_cancel(room: &str, thread_id: Uuid) {
    let Some(k) = Kernel::get() else {
        return;
    };
    let req = Frame::req(
        "chat:cancel",
        serde_json::json!({
            "room": room,
            "reply_to": thread_id.to_string(),
            "reason": "client_cancel",
        }),
    )
    .with_actor("system");

    let dispatcher = k.dispatcher().await;
    let mut rx = dispatcher.dispatch(
        req,
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        tokio_util::sync::CancellationToken::new(),
    );
    let _ = rx.recv().await;
}

// =============================================================================
// FRAME DETAIL
// =============================================================================

async fn handle_frame_detail(out_tx: &mpsc::Sender<WsOutMessage>, frame_id: &str) {
    let Some(k) = Kernel::get() else {
        return;
    };
    let Some(frames) = k.frames() else {
        return;
    };

    match read_frame_by_id(&frames, frame_id).await {
        Some(frame) => {
            let _ = out_tx
                .send(WsOutMessage::FrameDetail {
                    id: frame_id.to_string(),
                    data: frame.data,
                    trace: frame.trace,
                })
                .await;
        }
        None => {
            let _ = out_tx
                .send(WsOutMessage::Error {
                    message: format!("Frame not found: {}", frame_id),
                })
                .await;
        }
    }
}

async fn read_frame_by_id(store: &FrameStore, frame_id: &str) -> Option<Frame> {
    let row = sqlx::query_scalar::<_, String>(
        "SELECT frame_json FROM frames WHERE frame_id = ?1 LIMIT 1",
    )
    .bind(frame_id)
    .fetch_optional(store.pool())
    .await
    .ok()
    .flatten()?;

    serde_json::from_str(&row).ok()
}

// =============================================================================
// UTIL
// =============================================================================

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
