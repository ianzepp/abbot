// Shared handler for chat completions.
//
// Protocol-agnostic backend that:
// 1. Stores user message to SQLite
// 2. Publishes to head's scope
// 3. Wakes head immediately
// 4. Streams response chunks as head produces output

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::Stream;
use uuid::Uuid;

use crate::bus::{Message, MessageData, MessageOp, NeedPriority, Origin, Scope, respond};
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
    pub scope: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ChatChunk {
    Delta(String),
    ToolCall {
        tool_call_id: String,
        name: String,
        arguments_json: String,
    },
    Done,
    Error(String),
}

pub struct ChatHandler {
    bus: RuntimeBus,
    store: Arc<Store>,
}

impl ChatHandler {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, _head_id: impl Into<String>) -> Self {
        Self { bus, store }
    }

    pub async fn stream_existing(
        &self,
        scope: Scope,
        thread_id: Uuid,
    ) -> BoxStream<'static, ChatChunk> {
        let rx = self.bus.hub().read().await.subscribe_all();
        Box::pin(response_stream(rx, scope, thread_id))
    }

    pub async fn handle_chat(
        &self,
        request: ChatRequest,
    ) -> BoxStream<'static, ChatChunk> {
        // Extract <env>...</env> block from system message if present
        let env_block = request
            .messages
            .iter()
            .find(|m| matches!(m.role, Role::System))
            .and_then(|m| extract_env_block(&m.content));

        if let Some(ref env) = env_block {
            tracing::debug!(env = %env, "extracted env block from system prompt");
        }

        // Get the last user message
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

        // Build the message to send to the head
        let message_for_head = if let Some(env) = env_block {
            format!("{}\n\n{}", env, last_user_message)
        } else {
            last_user_message
        };

        tracing::debug!(
            content_len = message_for_head.len(),
            "user message for head"
        );

        let scope = request
            .scope
            .as_deref()
            .map(Scope::from)
            .unwrap_or_else(Scope::main);

        // Subscribe BEFORE publishing to avoid race condition
        let rx = self.bus.hub().read().await.subscribe_all();

        // Generate IDs for tracking
        let need_id = Uuid::new_v4().to_string();

        // Publish user message for history/logging
        let user_msg = respond::chat("_user", scope.clone(), &message_for_head)
            .with_origin(Origin::Human);
        let user_msg_id = user_msg.id;
        self.bus.publish(user_msg).await;

        let _ = self.store.set_active_thread(scope.as_str(), user_msg_id);

        // Create a need for NeedService to dispatch to a head.
        // Use the chat scope so the head can respond in-thread.
        let need_msg = respond::need_request(
            "_user",
            scope.clone(),
            &need_id,
            "user",
            NeedPriority::Normal,
            &message_for_head,
            "", // no additional context
        )
        .with_origin(Origin::Human)
        .with_reply_to(user_msg_id);

        self.bus.publish(need_msg).await;

        Box::pin(response_stream(rx, scope, user_msg_id))
    }
}

fn response_stream(
    rx: broadcast::Receiver<Message>,
    scope: Scope,
    user_msg_id: Uuid,
) -> impl Stream<Item = ChatChunk> + Send + 'static {
    let stream = BroadcastStream::new(rx);

    // Stream head chat messages until we receive Done/NeedMsg::Fulfilled, or an external tool request.
    let filtered = tokio_stream::StreamExt::filter_map(stream, move |result| {
            let Ok(msg) = result else {
                return None;
            };

            // Surface an external tool request and end the stream so the caller can execute it.
            if msg.op == MessageOp::Event {
                if msg.scope == scope && msg.reply_to == Some(user_msg_id) {
                    if let MessageData::Event { kind, payload } = &msg.data {
                        if kind == "external_tool_request" {
                            let tool_call_id = payload.get("tool_call_id")?.as_str()?.to_string();
                            let name = payload.get("name")?.as_str()?.to_string();
                            let arguments_json = payload
                                .get("arguments")
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}")
                                .to_string();
                            return Some(ChatChunk::ToolCall {
                                tool_call_id,
                                name,
                                arguments_json,
                            });
                        }
                    }
                }
            }

            // Check for Done signal with matching reply_to (chain complete)
            if msg.op == MessageOp::Done {
                if msg.reply_to == Some(user_msg_id) {
                    return Some(ChatChunk::Done);
                }
                return None;
            }

            // Check for need fulfillment (alternative completion signal)
            if msg.op == MessageOp::Need {
                if let MessageData::Need(crate::bus::NeedMsg::Fulfilled { .. }) = &msg.data {
                    if msg.reply_to == Some(user_msg_id) {
                        return Some(ChatChunk::Done);
                    }
                }
                return None;
            }

            // Filter for head chat messages
            if msg.op != MessageOp::Chat {
                return None;
            }

            if msg.origin != Origin::Head {
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
        .scan(false, |finished, chunk| {
            let out = if *finished { None } else { Some(chunk) };
            if matches!(out, Some(ChatChunk::Done) | Some(ChatChunk::ToolCall { .. })) {
                *finished = true;
            }
            std::future::ready(out)
        })
        .chain(tokio_stream::once(ChatChunk::Done));

    let timeout_stream = tokio_stream::StreamExt::timeout(filtered, Duration::from_secs(120));

    timeout_stream.map(|result| match result {
        Ok(chunk) => chunk,
        Err(_) => ChatChunk::Error("Response timeout".to_string()),
    })
}

fn extract_env_block(content: &str) -> Option<String> {
    let start = content.find("<env>")?;
    let end = content.find("</env>")?;
    if end <= start {
        return None;
    }
    Some(content[start..end + 6].to_string())
}
