// WebSocket handler for real-time updates to the web UI.
//
// Streams the full bus feed to connected clients. The frontend receives all
// messages and filters/processes them client-side as needed.

use axum::{
    extract::{
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::bus::Message;
use crate::runtime::RuntimeBus;

#[derive(Clone)]
pub struct WsState {
    pub bus: RuntimeBus,
}

impl WsState {
    pub fn new(bus: RuntimeBus) -> Self {
        Self { bus }
    }
}

// Outgoing message wrapper - all bus messages sent to frontend
#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
#[allow(dead_code)]
enum WsOutMessage {
    // Full bus message
    #[serde(rename = "bus")]
    Bus(BusMessageData),
    // Connection acknowledgment with server info
    #[serde(rename = "connected")]
    Connected { version: &'static str },
    // Heartbeat response (reserved for future use)
    #[serde(rename = "pong")]
    Pong { timestamp: u64 },
}

// Incoming message from client
#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsInMessage {
    // Heartbeat request
    #[serde(rename = "ping")]
    Ping,
}

// Bus message serialized for the frontend
#[derive(Serialize)]
struct BusMessageData {
    id: String,
    op: String,
    origin: String,
    sender: String,
    scope: String,
    data: serde_json::Value,
    reply_to: Option<String>,
    timestamp: i64,
}

impl From<Message> for BusMessageData {
    fn from(msg: Message) -> Self {
        let timestamp = msg
            .timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        Self {
            id: msg.id.to_string(),
            op: format!("{:?}", msg.op),
            origin: msg.origin.as_str().to_string(),
            sender: msg.sender,
            scope: msg.scope.to_string(),
            data: serde_json::to_value(&msg.data).unwrap_or(serde_json::Value::Null),
            reply_to: msg.reply_to.map(|u| u.to_string()),
            timestamp,
        }
    }
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<WsState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: WsState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Send connection acknowledgment
    let connected_msg = WsOutMessage::Connected { version: "0.1.0" };
    if let Ok(json) = serde_json::to_string(&connected_msg) {
        let _ = ws_sender.send(WsMessage::Text(json.into())).await;
    }

    // Subscribe to ALL bus messages
    let mut bus_rx = state.bus.hub().read().await.subscribe_all();

    // Forward bus messages to WebSocket
    let send_task = tokio::spawn(async move {
        loop {
            match bus_rx.recv().await {
                Ok(msg) => {
                    // Stream ALL messages to the frontend (no filtering)
                    let ws_msg = WsOutMessage::Bus(BusMessageData::from(msg));
                    let json = match serde_json::to_string(&ws_msg) {
                        Ok(j) => j,
                        Err(e) => {
                            warn!("Failed to serialize bus message: {}", e);
                            continue;
                        }
                    };

                    if ws_sender.send(WsMessage::Text(json.into())).await.is_err() {
                        debug!("WebSocket send failed, client disconnected");
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("WebSocket client lagged, dropped {} messages", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("Bus channel closed");
                    break;
                }
            }
        }
    });

    // Handle incoming messages from client
    while let Some(result) = ws_receiver.next().await {
        match result {
            Ok(WsMessage::Text(text)) => {
                if let Ok(msg) = serde_json::from_str::<WsInMessage>(&text) {
                    match msg {
                        WsInMessage::Ping => {
                            // Client requests heartbeat - handled by send_task's access
                            // For now we just log it; pong requires access to sender
                            debug!("Received ping from client");
                        }
                    }
                }
            }
            Ok(WsMessage::Ping(data)) => {
                // WebSocket-level ping is handled automatically by axum
                debug!("Received WebSocket ping: {:?}", data);
            }
            Ok(WsMessage::Close(_)) => {
                debug!("Client closed WebSocket connection");
                break;
            }
            Err(e) => {
                warn!("WebSocket receive error: {}", e);
                break;
            }
            _ => {}
        }
    }

    send_task.abort();
    debug!("WebSocket handler finished");
}
