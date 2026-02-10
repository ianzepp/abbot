//! WebSocket Handler - Bidirectional real-time channel for clients
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module implements the WebSocket ingress protocol for Abbot clients.
//! It is one of two protocol adapters (the other being OpenAI-compatible HTTP
//! in `openai.rs`), both of which ultimately dispatch the same `chat:message`
//! and `chat:cancel` syscalls through the kernel dispatcher.
//!
//! MESSAGE FLOW
//! ============
//! Inbound (client → server):
//!   ping         → pong (keepalive)
//!   chat.send    → dispatches `chat:message` syscall → spawns turn reader
//!   chat.cancel  → dispatches `chat:cancel` syscall → aborts turn reader
//!
//! Outbound (server → client):
//!   frame        → broadcast of ALL kernel frames (activity feed)
//!   chat.ack     → immediate acknowledgement with assigned thread_id
//!   chat.delta   → streaming text tokens from LLM
//!   chat.tool    → tool call notifications
//!   chat.done    → turn complete (terminal)
//!   chat.error   → turn error (terminal)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Turn stream opened BEFORE syscall dispatch (race prevention, same as handler.rs)
//! - At most one active turn per room (new send replaces previous turn tracking)
//! - Writer task decouples JSON serialization from the select loop
//! - Disconnect cleanup cancels all in-flight turns via `chat:cancel` syscall

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
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::debug;
use uuid::Uuid;

use crate::hal::llm::{ChatMessage, Role};
use crate::history::Store;
use crate::kernel::{Frame, FrameOp};
use crate::runtime::Kernel;

// =============================================================================
// STATE
// =============================================================================

/// Shared state injected into the WebSocket handler via Axum extractors.
///
/// WHY: Holds the history Store for persisting active thread IDs. The Store
/// is shared with the OpenAI adapter so both protocols write to the same
/// session state.
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
// WIRE PROTOCOL
// =============================================================================
//
// WHY: Tagged JSON enums provide a stable, typed contract between the Rust
// backend and the web UI. The `type` field acts as a discriminator so the
// browser can dispatch on message type without inspecting payload structure.
//
// DESIGN: Outbound messages use serde `tag = "type", content = "data"` so
// every message is `{ "type": "chat.delta", "data": { ... } }`. Inbound
// messages use `tag = "type"` with fields flattened into the root object.

/// Server → client message variants.
///
/// WHY: Each variant maps to a distinct client behavior: frame broadcast
/// updates the activity feed, chat.* variants drive the conversation panel,
/// and error/pong handle infrastructure concerns.
#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
enum WsOutMessage {
    #[serde(rename = "connected")]
    Connected { version: &'static str },

    #[serde(rename = "pong")]
    Pong { timestamp_ms: i64 },

    #[serde(rename = "frame")]
    Frame(Frame),

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

    #[serde(rename = "chat.status")]
    ChatStatus {
        room: String,
        thread_id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },

    #[serde(rename = "chat.mind")]
    ChatMind {
        room: String,
        actor: String,
        content: String,
    },

    #[serde(rename = "farewell")]
    Farewell { text: String },

    #[serde(rename = "error")]
    Error { message: String },
}

/// Client → server message variants.
///
/// WHY: Minimal inbound protocol — three message types. chat.send and
/// chat.cancel are the primary user interactions; ping is keepalive.
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

    #[serde(rename = "farewell.request")]
    FarewellRequest,
}

// =============================================================================
// ACTIVE TURN TRACKING
// =============================================================================
//
// WHY: Each room can have at most one active turn. The ActiveTurn tracks the
// thread_id (for cancellation) and the reader task handle (for abort on
// disconnect). When a new chat.send arrives for a room that already has an
// active turn, the old entry is removed (reader finishes naturally on
// chat:done). On WebSocket disconnect, ALL active turns are cancelled.

/// Tracks a single in-flight chat turn for a room.
///
/// WHY: Holds the thread_id needed for `chat:cancel` dispatch and the
/// JoinHandle needed to abort the reader task on disconnect cleanup.
struct ActiveTurn {
    thread_id: Uuid,
    reader_handle: JoinHandle<()>,
}

// =============================================================================
// HANDLER
// =============================================================================
//
// WHY: Two-function pattern — `ws_handler` validates the connection (loopback
// only) and upgrades HTTP → WebSocket; `handle_socket` runs the main event
// loop. Splitting these keeps Axum extractor concerns separate from protocol
// logic.

/// Axum handler for WebSocket upgrade requests.
///
/// SECURITY: Only accepts connections from loopback addresses. Abbot's HTTP
/// server is designed for local use only; this prevents remote clients from
/// connecting to the WebSocket endpoint.
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

/// Main WebSocket event loop.
///
/// WHY: Multiplexes three concerns in a single select loop:
/// 1. Inbound client messages (chat.send, chat.cancel, ping)
/// 2. Outbound frame broadcast (kernel activity feed)
/// 3. Outbound chat events (routed through per-turn reader tasks)
///
/// DESIGN: A dedicated writer task serializes WsOutMessage → JSON → WebSocket.
/// This decouples serialization from the select loop, preventing slow sends
/// from blocking frame or message processing.
///
/// LIFECYCLE:
/// - On connect: send "connected" + recent frame history (catchup)
/// - During session: select loop handles inbound + frame broadcast
/// - On disconnect: cancel all active turns, abort writer task
async fn handle_socket(socket: WebSocket, state: WsState) {
    let (ws_sender, mut ws_receiver) = socket.split();

    // -------------------------------------------------------------------------
    // PHASE 1: WRITER TASK SETUP
    // WHY: Decouples JSON serialization + WebSocket send from the select loop.
    // The out_tx channel is shared with turn reader tasks so they can push
    // chat.delta/chat.done/chat.error events without touching the socket.
    // -------------------------------------------------------------------------
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

    // -------------------------------------------------------------------------
    // PHASE 2: FRAME HISTORY CATCHUP
    // WHY: New clients need to see recent kernel activity so the UI isn't blank.
    // 100 frames is enough for the activity feed without overwhelming slow
    // connections.
    // -------------------------------------------------------------------------
    if let Some(frames) = k.frames()
        && let Ok(recent) = frames.read_recent(100).await
    {
        debug!(
            count = recent.len(),
            "sending recent frames to new websocket client"
        );
        for stored in recent {
            let _ = out_tx.send(WsOutMessage::Frame(stored.frame)).await;
        }
    }

    // -------------------------------------------------------------------------
    // PHASE 3: MAIN EVENT LOOP
    // WHY: Two-arm select multiplexes inbound client messages with the kernel's
    // frame broadcast. Chat events (delta/done/error) arrive via the out_tx
    // channel from per-turn reader tasks, not from the frame broadcast.
    // -------------------------------------------------------------------------
    let mut frame_rx = k.subscribe_frames().await;
    let mut active_turns: HashMap<String, ActiveTurn> = HashMap::new();

    loop {
        tokio::select! {
            // Inbound: client messages
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
                            Ok(WsInMessage::FarewellRequest) => {
                                let tx = out_tx.clone();
                                tokio::spawn(handle_farewell_request(tx));
                            }
                            Err(_) => {}
                        }
                    }
                    Ok(WsMessage::Close(_)) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }

            // Outbound: kernel frame broadcast (activity feed)
            frame = frame_rx.recv() => {
                match frame {
                    Ok(frame) => {
                        // Convert mind:thought frames to chat.mind messages
                        if frame.name.as_deref() == Some("mind:thought")
                            && let Some(data) = &frame.data
                        {
                            let room = data.get("room").and_then(|v| v.as_str()).unwrap_or("main");
                            let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                            if !content.is_empty() {
                                let _ = out_tx.send(WsOutMessage::ChatMind {
                                    room: room.to_string(),
                                    actor: frame.actor.clone().unwrap_or_default(),
                                    content: content.to_string(),
                                }).await;
                            }
                        }
                        // Send the raw frame for activity feed
                        if out_tx.send(WsOutMessage::Frame(frame)).await.is_err() {
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

    // -------------------------------------------------------------------------
    // PHASE 4: DISCONNECT CLEANUP
    // WHY: Any in-flight turns must be cancelled so the head agent stops
    // performing work for a disconnected client. Each active turn gets a
    // `chat:cancel` syscall dispatched through the kernel.
    // -------------------------------------------------------------------------
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
//
// WHY: This is the primary user interaction path. A chat.send message from
// the browser triggers: validation → turn stream setup → ack → syscall
// dispatch → reader task spawn. The ordering is critical — the turn stream
// must be opened BEFORE the syscall is dispatched to prevent lost frames.

/// Handle an inbound chat.send message from the browser.
///
/// WHY: Orchestrates the full user message → LLM response flow for WebSocket
/// clients. The sequence mirrors `ChatHandler::handle_chat()` in handler.rs
/// but uses WebSocket-native message types instead of SSE chunks.
///
/// DESIGN: The `chat:message` syscall is dispatched in a background task
/// (not awaited) so the select loop stays responsive for concurrent ping
/// and frame.detail messages during LLM processing.
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

    // WHY: Remove (but don't abort) the previous turn's reader. The reader
    // exits naturally when it sees chat:done or chat:error. Dropping the
    // JoinHandle does NOT abort the spawned task in tokio — the reader
    // continues to drain its stream without sending to a disconnected client.
    active_turns.remove(&room);

    let thread_id = Uuid::new_v4();

    // WHY: Open turn stream BEFORE dispatching the syscall. If we dispatch
    // first, the head agent could emit frames before we're listening.
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

    // WHY: Dispatch in background (not awaited) so the select loop stays
    // responsive for concurrent ping and frame.detail messages. The syscall
    // may block on the room mutex if a previous turn is still running.
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

        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mut dispatch_rx =
            k.dispatcher()
                .await
                .dispatch(req, cwd, tokio_util::sync::CancellationToken::new());
        tokio::spawn(async move {
            let _ = dispatch_rx.recv().await;
        });
    }

    // WHY: Spawn a dedicated reader task per turn. This task converts raw
    // sigcall frames into typed WsOutMessage variants (delta/tool/done/error)
    // and pushes them through the shared out_tx channel to the writer task.
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

/// Read frames from a turn's sigcall stream and convert to WebSocket events.
///
/// WHY: Each active turn gets its own reader task that bridges the kernel's
/// frame-based sigcall stream to the WebSocket wire protocol. The reader
/// exits on terminal events (chat.done, chat.error) or when the out_tx
/// channel closes (WebSocket disconnected).
///
/// FRAME → WS MESSAGE MAPPING:
/// - FrameOp::Item + type="text_delta" → chat.delta
/// - FrameOp::Item + type="tool_call"  → chat.tool
/// - FrameOp::Item + type="done"       → chat.done (terminal)
/// - FrameOp::Error                    → chat.error (terminal)
/// - FrameOp::Done                     → chat.done (terminal, fallback)
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
                    Some("status") => {
                        let status = data
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let actor = data
                            .get("actor")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let tool = data
                            .get("tool")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let summary = data
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        WsOutMessage::ChatStatus {
                            room: room.clone(),
                            thread_id: tid.clone(),
                            status,
                            actor,
                            tool,
                            summary,
                        }
                    }
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
//
// WHY: Two cancellation paths exist:
// 1. Explicit: client sends chat.cancel → handle_chat_cancel
// 2. Implicit: WebSocket disconnect → handle_socket cleanup loop
// Both use dispatch_cancel to send `chat:cancel` syscall to the kernel,
// which marks the turn as cancelled so the head agent stops working.

/// Handle an explicit chat.cancel message from the browser.
///
/// WHY: User clicked "stop" in the UI. Aborts the reader task immediately
/// (no more events sent to client) and dispatches `chat:cancel` so the
/// head agent observes cancellation on its next tool dispatch attempt.
async fn handle_chat_cancel(active_turns: &mut HashMap<String, ActiveTurn>, room: &str) {
    if let Some(turn) = active_turns.remove(room) {
        turn.reader_handle.abort();
        dispatch_cancel(room, turn.thread_id).await;
    }
}

/// Dispatch `chat:cancel` syscall to the kernel for a specific turn.
///
/// WHY: Centralized cancellation dispatch used by both explicit cancel and
/// disconnect cleanup. Uses "client_cancel" reason (vs "client_disconnect"
/// used by handler.rs's CancelOnDropStream) so log analysis can distinguish
/// the two cancellation sources.
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
// FAREWELL
// =============================================================================

/// Generate a dynamic farewell message via LLM.
///
/// WHY: Instead of showing a random static line from farewells.txt, ask the
/// LLM for a fresh zen farewell so each exit feels unique. Fire-and-forget
/// from the client's perspective — if anything fails, silently return and the
/// client falls back to a static farewell.
async fn handle_farewell_request(out_tx: mpsc::Sender<WsOutMessage>) {
    let Some(k) = Kernel::get() else {
        return;
    };

    let messages = vec![
        ChatMessage::new(
            Role::System,
            "You write single-line zen farewell messages for a CLI tool called Abbot \
             (an octopus monk who codes). The tone is calm, wise, slightly whimsical. \
             Themes: coding, monasteries, tentacles, git, deploys, rest. \
             Respond with exactly one short sentence — nothing else.",
        ),
        ChatMessage::new(Role::User, "Write a farewell."),
    ];

    let payload = serde_json::json!({ "messages": messages });
    let req = Frame::req("llm:chat", payload).with_actor("hand/farewell");
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let dispatcher = k.dispatcher().await;
    let mut rx = dispatcher.dispatch(req, cwd, tokio_util::sync::CancellationToken::new());

    let mut content = String::new();
    while let Some(frame) = rx.recv().await {
        match frame.op {
            FrameOp::Item => {
                if let Some(data) = frame.data
                    && data.get("type").and_then(|v| v.as_str()) == Some("text_delta")
                    && let Some(text) = data.get("content").and_then(|v| v.as_str())
                {
                    content.push_str(text);
                }
            }
            FrameOp::Done => break,
            FrameOp::Error => return,
            _ => {}
        }
    }

    let text = content.trim().to_string();
    if !text.is_empty() {
        let _ = out_tx.send(WsOutMessage::Farewell { text }).await;
    }
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
