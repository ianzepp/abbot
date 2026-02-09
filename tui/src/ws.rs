//! WebSocket client — connects to daemon, sends/receives chat messages.

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

// =============================================================================
// WIRE TYPES (must match daemon/src/server/websocket.rs)
// =============================================================================

#[derive(Serialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
pub enum WsInMessage {
    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "chat.send")]
    ChatSend {
        scope: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },

    #[serde(rename = "chat.cancel")]
    ChatCancel { scope: String },
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
    Frame(WireFrame),

    #[serde(rename = "chat.ack")]
    ChatAck {
        scope: String,
        #[allow(dead_code)]
        thread_id: String,
        #[allow(dead_code)]
        client_id: Option<String>,
    },

    #[serde(rename = "chat.delta")]
    ChatDelta {
        scope: String,
        #[allow(dead_code)]
        thread_id: String,
        content: String,
    },

    #[serde(rename = "chat.tool")]
    ChatTool {
        scope: String,
        #[allow(dead_code)]
        thread_id: String,
        #[allow(dead_code)]
        tool_call_id: String,
        name: String,
        #[allow(dead_code)]
        arguments: String,
    },

    #[serde(rename = "chat.done")]
    ChatDone {
        scope: String,
        #[allow(dead_code)]
        thread_id: String,
        #[allow(dead_code)]
        reason: String,
    },

    #[serde(rename = "chat.error")]
    ChatError {
        scope: String,
        #[allow(dead_code)]
        thread_id: String,
        #[allow(dead_code)]
        code: String,
        message: String,
    },

    #[serde(rename = "error")]
    Error {
        #[allow(dead_code)]
        message: String,
    },

    #[serde(rename = "frame.detail")]
    #[allow(dead_code)]
    FrameDetail {
        id: String,
        data: Option<serde_json::Value>,
        trace: Option<serde_json::Value>,
    },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct WireFrame {
    pub id: String,
    pub ts: i64,
    pub op: String,
    pub name: Option<String>,
    pub scope: Option<String>,
    pub summary: String,
    #[allow(dead_code)]
    pub actor: Option<String>,
    #[allow(dead_code)]
    pub parent_id: Option<String>,
}

// =============================================================================
// EVENTS (background task → main loop)
// =============================================================================

pub enum WsEvent {
    Connected,
    Disconnected,
    ChatAck { scope: String },
    ChatDelta { scope: String, content: String },
    ChatTool { scope: String, name: String },
    ChatDone { scope: String },
    ChatError { scope: String, message: String },
    Frame(WireFrame),
}

// =============================================================================
// BACKGROUND TASK
// =============================================================================

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

        loop {
            tokio::select! {
                msg = stream.next() => {
                    let Some(msg) = msg else { break };
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(out) = serde_json::from_str::<WsOutMessage>(&text) {
                                match out {
                                    WsOutMessage::Connected { .. } | WsOutMessage::Pong { .. } => {}
                                    WsOutMessage::ChatAck { scope, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatAck { scope }).await;
                                    }
                                    WsOutMessage::ChatDelta { scope, content, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatDelta { scope, content }).await;
                                    }
                                    WsOutMessage::ChatTool { scope, name, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatTool { scope, name }).await;
                                    }
                                    WsOutMessage::ChatDone { scope, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatDone { scope }).await;
                                    }
                                    WsOutMessage::ChatError { scope, message, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatError { scope, message }).await;
                                    }
                                    WsOutMessage::Frame(frame) => {
                                        let _ = event_tx.send(WsEvent::Frame(frame)).await;
                                    }
                                    WsOutMessage::Error { .. } | WsOutMessage::FrameDetail { .. } => {}
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
