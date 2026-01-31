// WebSocket handler for real-time updates to the web UI.
//
// Subscribes to the message bus and forwards relevant messages to connected
// clients. Also sends periodic status updates.

use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::broadcast;

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

#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
enum WsOutMessage {
    #[serde(rename = "message")]
    Message(WsMessageData),
}

#[derive(Serialize)]
struct WsMessageData {
    id: String,
    op: String,
    origin: String,
    sender: String,
    scope: String,
    data: serde_json::Value,
    reply_to: Option<String>,
    timestamp: i64,
}

impl From<Message> for WsMessageData {
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

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: WsState) {
    let (mut sender, mut receiver) = socket.split();
    
    // Subscribe to bus messages
    let mut rx = state.bus.hub().read().await.subscribe_all();

    // Spawn task to forward bus messages to WebSocket
    let send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    // Only forward Chat messages to the UI for now
                    if msg.op != crate::bus::MessageOp::Chat {
                        continue;
                    }

                    let ws_msg = WsOutMessage::Message(WsMessageData::from(msg));
                    let json = match serde_json::to_string(&ws_msg) {
                        Ok(j) => j,
                        Err(_) => continue,
                    };

                    if sender.send(WsMessage::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Handle incoming messages from client (for future use)
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(WsMessage::Text(_text)) => {
                // Handle client messages if needed
            }
            Ok(WsMessage::Close(_)) => break,
            Err(_) => break,
            _ => {}
        }
    }

    send_task.abort();
}
