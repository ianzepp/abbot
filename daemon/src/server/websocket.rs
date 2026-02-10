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
//!   frame        → dispatches Frame through kernel dispatcher
//!
//! Outbound (server → client):
//!   connected    → initial handshake
//!   frame        → broadcast of ALL kernel frames
//!   error        → protocol-level errors
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

use crate::history::Store;
use crate::kernel::{Frame, FrameOp};
use crate::runtime::Kernel;

// =============================================================================
// STATE
// =============================================================================

/// Shared state injected into the WebSocket handler via Axum extractors.
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

/// Server → client message variants.
#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
enum WsOutMessage {
    #[serde(rename = "connected")]
    Connected { version: &'static str },

    #[serde(rename = "pong")]
    Pong { timestamp_ms: i64 },

    #[serde(rename = "frame")]
    Frame(Frame),

    #[serde(rename = "error")]
    Error { message: String },
}

/// Client → server message variants.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsInMessage {
    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "frame")]
    FrameMsg { frame: Frame },
}

// =============================================================================
// ACTIVE TURN TRACKING
// =============================================================================

/// Tracks a single in-flight chat turn for a room.
struct ActiveTurn {
    thread_id: Uuid,
    reader_handle: JoinHandle<()>,
}

// =============================================================================
// HANDLER
// =============================================================================

/// Axum handler for WebSocket upgrade requests.
///
/// SECURITY: Only accepts connections from loopback addresses.
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
async fn handle_socket(socket: WebSocket, state: WsState) {
    let (ws_sender, mut ws_receiver) = socket.split();

    // Writer task: decouples JSON serialization from the select loop.
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

    // Frame history catchup: send recent frames so the UI isn't blank.
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

    // Main event loop: inbound client messages + kernel frame broadcast.
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
                            Ok(WsInMessage::FrameMsg { frame }) => {
                                handle_inbound_frame(
                                    &state,
                                    &out_tx,
                                    &mut active_turns,
                                    frame,
                                ).await;
                            }
                            Err(_) => {}
                        }
                    }
                    Ok(WsMessage::Close(_)) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }

            // Outbound: kernel frame broadcast
            frame = frame_rx.recv() => {
                match frame {
                    Ok(frame) => {
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

    // Disconnect cleanup: cancel all in-flight turns.
    for (room, turn) in active_turns.drain() {
        turn.reader_handle.abort();
        dispatch_cancel(&room, turn.thread_id).await;
    }

    writer_handle.abort();
    debug!("websocket handler finished");
}

// =============================================================================
// INBOUND FRAME ROUTING
// =============================================================================

/// Route an inbound frame by its name to the appropriate handler.
async fn handle_inbound_frame(
    state: &WsState,
    out_tx: &mpsc::Sender<WsOutMessage>,
    active_turns: &mut HashMap<String, ActiveTurn>,
    frame: Frame,
) {
    match frame.name.as_deref() {
        Some("chat:message") => {
            handle_chat_message_frame(state, out_tx, active_turns, frame).await;
        }
        Some("chat:cancel") => {
            handle_chat_cancel_frame(active_turns, &frame).await;
        }
        Some(_) => {
            handle_generic_dispatch(out_tx, frame).await;
        }
        None => {}
    }
}

// =============================================================================
// CHAT MESSAGE
// =============================================================================

/// Handle an inbound chat:message frame from the client.
async fn handle_chat_message_frame(
    state: &WsState,
    out_tx: &mpsc::Sender<WsOutMessage>,
    active_turns: &mut HashMap<String, ActiveTurn>,
    frame: Frame,
) {
    let room = frame
        .data
        .as_ref()
        .and_then(|d| d.get("room"))
        .and_then(|v| v.as_str())
        .unwrap_or("main")
        .to_string();

    let text = frame
        .data
        .as_ref()
        .and_then(|d| d.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

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

    // Remove (but don't abort) the previous turn's reader.
    active_turns.remove(&room);

    let thread_id = Uuid::new_v4();

    // Open turn stream BEFORE dispatching the syscall (race prevention).
    let rx = k.sigcalls().open(&room, thread_id).await;
    let _ = state.store.set_active_thread(&room, thread_id).await;

    // Emit synthetic ack event as a Frame.
    let ack = Frame::event(
        Uuid::nil(),
        serde_json::json!({
            "kind": "chat.ack",
            "room": &room,
            "thread_id": thread_id.to_string(),
        }),
    )
    .with_name("chat:ack");
    let _ = out_tx.send(WsOutMessage::Frame(ack)).await;

    // Dispatch the chat:message frame through the kernel.
    {
        let mut dispatch_frame = frame;
        // Ensure reply_to is set for turn tracking.
        if let Some(data) = dispatch_frame.data.as_mut()
            && let Some(obj) = data.as_object_mut()
        {
            obj.insert(
                "reply_to".to_string(),
                serde_json::json!(thread_id.to_string()),
            );
        }

        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mut dispatch_rx = k.dispatcher().await.dispatch(
            dispatch_frame,
            cwd,
            tokio_util::sync::CancellationToken::new(),
        );
        tokio::spawn(async move {
            let _ = dispatch_rx.recv().await;
        });
    }

    // Spawn turn reader that forwards raw frames.
    let reader_out_tx = out_tx.clone();
    let reader_handle = tokio::spawn(async move {
        turn_stream_reader(rx, reader_out_tx).await;
    });

    active_turns.insert(
        room,
        ActiveTurn {
            thread_id,
            reader_handle,
        },
    );
}

/// Read frames from a turn's sigcall stream and forward as raw frames.
/// Exits on terminal ops (Done/Error) or when the channel closes.
async fn turn_stream_reader(mut rx: mpsc::Receiver<Frame>, out_tx: mpsc::Sender<WsOutMessage>) {
    while let Some(frame) = rx.recv().await {
        let is_terminal = matches!(frame.op, FrameOp::Done | FrameOp::Error);
        if out_tx.send(WsOutMessage::Frame(frame)).await.is_err() {
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

/// Handle an inbound chat:cancel frame from the client.
async fn handle_chat_cancel_frame(active_turns: &mut HashMap<String, ActiveTurn>, frame: &Frame) {
    let room = frame
        .data
        .as_ref()
        .and_then(|d| d.get("room"))
        .and_then(|v| v.as_str())
        .unwrap_or("main");

    if let Some(turn) = active_turns.remove(room) {
        turn.reader_handle.abort();
        dispatch_cancel(room, turn.thread_id).await;
    }
}

/// Dispatch `chat:cancel` syscall to the kernel for a specific turn.
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
// GENERIC DISPATCH
// =============================================================================

/// Dispatch any non-chat frame through the kernel and forward response frames.
async fn handle_generic_dispatch(out_tx: &mpsc::Sender<WsOutMessage>, frame: Frame) {
    let Some(k) = Kernel::get() else {
        return;
    };

    let out_tx = out_tx.clone();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut rx =
        k.dispatcher()
            .await
            .dispatch(frame, cwd, tokio_util::sync::CancellationToken::new());

    tokio::spawn(async move {
        while let Some(response_frame) = rx.recv().await {
            let is_terminal = matches!(response_frame.op, FrameOp::Done | FrameOp::Error);
            if out_tx
                .send(WsOutMessage::Frame(response_frame))
                .await
                .is_err()
            {
                break;
            }
            if is_terminal {
                break;
            }
        }
    });
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
