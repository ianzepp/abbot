// Streams kernel frame audit logs to connected clients.

use axum::{
    extract::{
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt, future::pending};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::kernel::{Frame, FrameOp};
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
    Frame(LoggedFrameData),

    #[serde(rename = "pong")]
    Pong { timestamp_ms: i64 },

    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Serialize)]
struct LoggedFrameData {
    seq: u64,
    ts_ms: i64,
    frame: Frame,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsInMessage {
    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "tail")]
    Tail {
        #[serde(default)]
        since: Option<u64>,
        #[serde(default)]
        limit: Option<u64>,
    },
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

    // Default: start tailing from near the end.
    let default_since = k
        .audit()
        .map(|a| a.last_seq().saturating_sub(200))
        .unwrap_or(0);

    let mut tail_since = default_since;
    let mut tail_limit: u64 = 200;
    let mut tail_rx: Option<crate::kernel::KernelReceiver> = None;
    let mut tail_cancel: Option<tokio_util::sync::CancellationToken> = None;

    start_tail(
        &k,
        &mut tail_rx,
        &mut tail_cancel,
        tail_since,
        tail_limit,
    )
    .await;

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
                                let _ = ws_sender.send(WsMessage::Text(
                                    serde_json::to_string(&WsOutMessage::Pong { timestamp_ms: now_ms() }).unwrap_or_default().into()
                                )).await;
                            }
                            Ok(WsInMessage::Tail { since, limit }) => {
                                tail_since = since.unwrap_or(0);
                                tail_limit = limit.unwrap_or(200).clamp(1, 2000);
                                start_tail(&k, &mut tail_rx, &mut tail_cancel, tail_since, tail_limit).await;
                            }
                            Err(e) => {
                                warn!(error = %e, "ws invalid message");
                            }
                        }
                    }
                    Ok(WsMessage::Close(_)) => break,
                    Ok(WsMessage::Ping(_)) => {}
                    Ok(_) => {}
                    Err(e) => {
                        warn!(error = %e, "ws receive error");
                        break;
                    }
                }
            }

            frame = async {
                if let Some(rx) = tail_rx.as_mut() {
                    rx.recv().await
                } else {
                    pending::<Option<Frame>>().await
                }
            } => {
                let Some(frame) = frame else {
                    // Tail stream ended; keep the socket open.
                    tail_rx = None;
                    continue;
                };

                if frame.op != FrameOp::Item {
                    continue;
                }
                let Some(data) = frame.data else {
                    continue;
                };

                let seq = data.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
                let ts_ms = data.get("ts_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                let frame_val = data.get("frame").cloned().unwrap_or(serde_json::Value::Null);
                let parsed: Result<Frame, _> = serde_json::from_value(frame_val);
                let frame = match parsed {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                let out = WsOutMessage::Frame(LoggedFrameData { seq, ts_ms, frame });
                let json = match serde_json::to_string(&out) {
                    Ok(j) => j,
                    Err(e) => {
                        warn!(error = %e, "ws serialize failed");
                        continue;
                    }
                };
                if ws_sender.send(WsMessage::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    }

    if let Some(c) = tail_cancel.take() {
        c.cancel();
    }
    debug!("websocket handler finished");
}

async fn start_tail(
    k: &Kernel,
    tail_rx: &mut Option<crate::kernel::KernelReceiver>,
    tail_cancel: &mut Option<tokio_util::sync::CancellationToken>,
    since: u64,
    limit: u64,
) {
    if let Some(c) = tail_cancel.take() {
        c.cancel();
    }

    let dispatcher = k.dispatcher().await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let req = crate::kernel::Frame::req(
        "log:tail",
        serde_json::json!({"since": since, "limit": limit}),
    )
    .with_actor("ws/client");
    let rx = dispatcher.dispatch(req, k.workspace().to_path_buf(), cancel.clone());
    *tail_cancel = Some(cancel);
    *tail_rx = Some(rx);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
