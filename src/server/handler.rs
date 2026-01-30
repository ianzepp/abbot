// Shared handler for chat completions.
//
// Protocol-agnostic backend that:
// 1. Stores user message to SQLite
// 2. Publishes to head's scope
// 3. Wakes head immediately
// 4. Streams response chunks as head produces output

use std::sync::Arc;
use std::time::Duration;

use futures::stream::BoxStream;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope, respond};
use crate::history::Store;
use crate::runtime::RuntimeBus;

#[derive(Debug, Clone)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
}

#[derive(Debug, Clone)]
pub enum ChatChunk {
    Delta(String),
    Done,
    Error(String),
}

pub struct ChatHandler {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
}

impl ChatHandler {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, head_id: impl Into<String>) -> Self {
        Self {
            bus,
            store,
            head_id: head_id.into(),
        }
    }

    pub async fn handle_chat(
        &self,
        request: ChatRequest,
    ) -> BoxStream<'static, ChatChunk> {
        let last_user_message = request
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, Role::User))
            .map(|m| m.content.clone())
            .unwrap_or_default();

        if last_user_message.is_empty() {
            return Box::pin(tokio_stream::once(ChatChunk::Error(
                "No user message provided".to_string(),
            )));
        }

        let main_scope = Scope::main();
        let head_mail = Scope::head_mail(&self.head_id);

        let user_msg = respond::chat("_user", main_scope.clone(), &last_user_message)
            .with_origin(Origin::Human);
        self.bus.publish(user_msg).await;

        let wake_msg = respond::wake("_server", head_mail, 0).with_origin(Origin::System);
        self.bus.publish(wake_msg).await;

        let rx = self.bus.hub().read().await.subscribe_all();
        let head_id = self.head_id.clone();

        Box::pin(response_stream(rx, head_id, main_scope))
    }
}

fn response_stream(
    rx: broadcast::Receiver<Message>,
    head_id: String,
    scope: Scope,
) -> impl Stream<Item = ChatChunk> + Send + 'static {
    let stream = BroadcastStream::new(rx);

    let filtered = stream
        .filter_map(move |result| {
            let Ok(msg) = result else {
                return None;
            };

            if msg.op != MessageOp::Chat {
                return None;
            }

            if msg.origin != Origin::Head {
                return None;
            }

            if msg.sender != head_id {
                return None;
            }

            if msg.scope != scope {
                return None;
            }

            let MessageData::Text(content) = msg.data else {
                return None;
            };

            Some(ChatChunk::Delta(content))
        })
        .take(1)
        .chain(tokio_stream::once(ChatChunk::Done));

    let timeout_stream = tokio_stream::StreamExt::timeout(filtered, Duration::from_secs(120));

    timeout_stream.map(|result| match result {
        Ok(chunk) => chunk,
        Err(_) => ChatChunk::Error("Response timeout".to_string()),
    })
}
