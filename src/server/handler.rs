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
use uuid::Uuid;

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
        // Count message types for debugging
        let system_count = request.messages.iter().filter(|m| matches!(m.role, Role::System)).count();
        let user_count = request.messages.iter().filter(|m| matches!(m.role, Role::User)).count();
        let assistant_count = request.messages.iter().filter(|m| matches!(m.role, Role::Assistant)).count();

        tracing::debug!(
            system_count = %system_count,
            user_count = %user_count,
            assistant_count = %assistant_count,
            "processing chat request (NOTE: system messages are currently ignored)"
        );

        // Log all messages for debugging
        for (i, msg) in request.messages.iter().enumerate() {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let preview: String = msg.content.chars().take(200).collect();
            tracing::debug!(
                index = i,
                role = role,
                content_len = msg.content.len(),
                preview = %preview,
                "message"
            );
        }

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

        // Subscribe BEFORE publishing to avoid race condition
        let rx = self.bus.hub().read().await.subscribe_all();
        let head_id = self.head_id.clone();

        let user_msg = respond::chat("_user", main_scope.clone(), &last_user_message)
            .with_origin(Origin::Human);
        let user_msg_id = user_msg.id;
        self.bus.publish(user_msg).await;

        Box::pin(response_stream(rx, head_id, main_scope, user_msg_id))
    }
}

fn response_stream(
    rx: broadcast::Receiver<Message>,
    head_id: String,
    scope: Scope,
    user_msg_id: Uuid,
) -> impl Stream<Item = ChatChunk> + Send + 'static {
    let stream = BroadcastStream::new(rx);

    // Stream head chat messages until we receive Done (for this reply chain) or Idle
    let filtered = stream
        .filter_map(move |result| {
            let Ok(msg) = result else {
                return None;
            };

            // Check for Done signal with matching reply_to (chain complete)
            if msg.op == MessageOp::Done {
                if msg.reply_to == Some(user_msg_id) {
                    return Some(ChatChunk::Done);
                }
                return None;
            }

            // Ignore Idle here. Idle is global, Done is per-reply chain.

            // Filter for head chat messages
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

            // Only stream messages for this reply chain.
            if msg.reply_to != Some(user_msg_id) {
                return None;
            }

            let MessageData::Text(content) = msg.data else {
                return None;
            };

            // Separate multiple head messages with newline
            Some(ChatChunk::Delta(format!("{}\n", content)))
        })
        .take_while(|chunk| !matches!(chunk, ChatChunk::Done))
        .chain(tokio_stream::once(ChatChunk::Done));

    let timeout_stream = tokio_stream::StreamExt::timeout(filtered, Duration::from_secs(120));

    timeout_stream.map(|result| match result {
        Ok(chunk) => chunk,
        Err(_) => ChatChunk::Error("Response timeout".to_string()),
    })
}
