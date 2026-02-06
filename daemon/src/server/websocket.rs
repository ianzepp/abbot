// Streams kernel frames to connected clients via broadcast channel.

use axum::{
    extract::{
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::kernel::Frame;
use crate::runtime::Kernel;

#[derive(Clone)]
pub struct WsState {}

impl WsState {
    pub fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
enum WsOutMessage {
    #[serde(rename = "connected")]
    Connected { version: &'static str },

    #[serde(rename = "frame")]
    Frame(Frame),

    #[serde(rename = "pong")]
    Pong { timestamp_ms: i64 },

    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsInMessage {
    #[serde(rename = "ping")]
    Ping,
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<WsState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, _state: WsState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    let connected_msg = WsOutMessage::Connected { version: "0.1.0" };
    if let Ok(json) = serde_json::to_string(&connected_msg) {
        let _ = ws_sender.send(WsMessage::Text(json.into())).await;
    }

    let Some(k) = Kernel::get() else {
        let out = WsOutMessage::Error {
            message: "Kernel not initialized".to_string(),
        };
        if let Ok(json) = serde_json::to_string(&out) {
            let _ = ws_sender.send(WsMessage::Text(json.into())).await;
        }
        return;
    };

    // Send recent frames to give the client an initial dataset
    if let Some(store) = k.frames() {
        if let Ok(recent) = store.read_recent(100).await {
            debug!(count = recent.len(), "sending recent frames to new websocket client");
            for logged in recent {
                let out = WsOutMessage::Frame(logged.frame);
                if let Ok(json) = serde_json::to_string(&out) {
                    if ws_sender.send(WsMessage::Text(json.into())).await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    let mut frame_rx = k.subscribe_frames().await;

    loop {
        tokio::select! {
            msg = ws_receiver.next() => {
                let Some(msg) = msg else {
                    break;
                };
                match msg {
                    Ok(WsMessage::Text(text)) => {
                        match serde_json::from_str::<WsInMessage>(&text) {
                            Ok(WsInMessage::Ping) => {
                                let pong = WsOutMessage::Pong { timestamp_ms: now_ms() };
                                if let Ok(json) = serde_json::to_string(&pong) {
                                    let _ = ws_sender.send(WsMessage::Text(json.into())).await;
                                }
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
                        let out = WsOutMessage::Frame(frame);
                        let json = match serde_json::to_string(&out) {
                            Ok(j) => j,
                            Err(_) => continue,
                        };
                        if ws_sender.send(WsMessage::Text(json.into())).await.is_err() {
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

    debug!("websocket handler finished");
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
