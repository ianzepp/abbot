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
        room: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },

    #[serde(rename = "chat.cancel")]
    ChatCancel { room: String },
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
        room: String,
        #[allow(dead_code)]
        thread_id: String,
        #[allow(dead_code)]
        client_id: Option<String>,
    },

    #[serde(rename = "chat.delta")]
    ChatDelta {
        room: String,
        #[allow(dead_code)]
        thread_id: String,
        content: String,
    },

    #[serde(rename = "chat.tool")]
    ChatTool {
        room: String,
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
        room: String,
        #[allow(dead_code)]
        thread_id: String,
        #[allow(dead_code)]
        reason: String,
    },

    #[serde(rename = "chat.error")]
    ChatError {
        room: String,
        #[allow(dead_code)]
        thread_id: String,
        #[allow(dead_code)]
        code: String,
        message: String,
    },

    #[serde(rename = "chat.status")]
    ChatStatus {
        room: String,
        #[allow(dead_code)]
        thread_id: String,
        status: String,
        #[allow(dead_code)]
        actor: Option<String>,
        tool: Option<String>,
        summary: Option<String>,
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
    pub room: Option<String>,
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
    ChatAck {
        room: String,
    },
    ChatDelta {
        room: String,
        content: String,
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
    },
    Frame(WireFrame),
    ChatReplay {
        room: String,
        entries: Vec<crate::replay::ReplayEntry>,
    },
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
                                    WsOutMessage::ChatAck { room, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatAck { room }).await;
                                    }
                                    WsOutMessage::ChatDelta { room, content, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatDelta { room, content }).await;
                                    }
                                    WsOutMessage::ChatTool { room, name, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatTool { room, name }).await;
                                    }
                                    WsOutMessage::ChatDone { room, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatDone { room }).await;
                                    }
                                    WsOutMessage::ChatError { room, message, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatError { room, message }).await;
                                    }
                                    WsOutMessage::ChatStatus { room, status, actor, tool, summary, .. } => {
                                        let _ = event_tx.send(WsEvent::ChatStatus { room, status, actor, tool, summary }).await;
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
