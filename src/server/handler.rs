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
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::Scope;
use crate::history::Store;
use crate::kernel::{Frame, FrameOp};
use crate::runtime::Kernel;

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
    store: Arc<Store>,
}

impl ChatHandler {
    pub fn new(store: Arc<Store>, _head_id: impl Into<String>) -> Self {
        Self { store }
    }

    pub async fn stream_existing(
        &self,
        scope: Scope,
        thread_id: Uuid,
    ) -> BoxStream<'static, ChatChunk> {
        let Some(k) = Kernel::get() else {
            return Box::pin(tokio_stream::once(ChatChunk::Error(
                "Kernel not initialized".to_string(),
            )));
        };
        let rx = k.sigcalls().open(scope.as_str(), thread_id).await;
        Box::pin(response_stream(rx))
    }

    pub async fn handle_chat(&self, request: ChatRequest) -> BoxStream<'static, ChatChunk> {
        // Extract <env>...</env> block from system message if present
        let env_block = request
            .messages
            .iter()
            .find(|m| matches!(m.role, Role::System))
            .and_then(|m| extract_env_block(&m.content));

        if let Some(ref env) = env_block {
            tracing::debug!(env = %env, "extracted env block from system prompt");
        }

        let non_system: Vec<&ChatMessage> = request
            .messages
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .collect();

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

        // Heuristic: create a checkpoint when the client only sends a single user message
        // (common when starting a fresh session without history). This lets the user effectively
        // start a new conversation without deleting logs.
        let mut reset = false;
        if let Some((only,)) = non_system.as_slice().split_first().and_then(|(a, rest)| {
            if rest.is_empty() { Some((a,)) } else { None }
        }) {
            if matches!(only.role, Role::User) {
                // Default to enabled; allow disabling via config.
                let enabled = crate::runtime::AppConfig::global()
                    .server
                    .reset_on_single_user_message
                    .unwrap_or(true);
                if enabled {
                    reset = true;
                    tracing::debug!(scope = %request.scope.as_deref().unwrap_or("main"), "single-message request; inserted chat reset checkpoint");
                }
            }
        }

        // Explicit reset command.
        let mut message_for_head = last_user_message;
        let trimmed = message_for_head.trim_start();
        if let Some(rest) = trimmed.strip_prefix("/reset") {
            reset = true;
            message_for_head = rest.trim_start().to_string();
        }

        // Build the message to send to the head.
        // Session environment is persisted separately and injected into the head's system prompt.


        tracing::debug!(
            content_len = message_for_head.len(),
            "user message for head"
        );

        let scope = request
            .scope
            .as_deref()
            .map(Scope::from)
            .unwrap_or_else(Scope::main);

        if let Some(ref env) = env_block {
            let _ = self.store.set_session_env(scope.as_str(), env);
        }

        let Some(k) = Kernel::get() else {
            return Box::pin(tokio_stream::once(ChatChunk::Error(
                "Kernel not initialized".to_string(),
            )));
        };

        // Generate IDs for tracking
        let need_id = Uuid::new_v4().to_string();

        // Thread id for correlating reply stream + logging.
        let user_msg_id = Uuid::new_v4();

        // Open reply stream BEFORE publishing need to avoid races.
        let rx = k.sigcalls().open(scope.as_str(), user_msg_id).await;

        // Best-effort log of the user message into logs.db.
        {
            let dispatcher = k.dispatcher().await;

            if reset {
                let req = Frame::req(
                    "log:append",
                    serde_json::json!({
                        "kind": "chat:reset",
                        "scope": scope.as_str(),
                        "data": {"reason": "client_reset"}
                    }),
                )
                .with_actor("human/_user");
                let mut rx_reset = dispatcher.dispatch(
                    req,
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                    tokio_util::sync::CancellationToken::new(),
                );
                let _ = rx_reset.recv().await;
            }

            let req = Frame::req(
                "log:append",
                serde_json::json!({
                    "kind": "chat:user",
                    "scope": scope.as_str(),
                    "data": {
                        "content": message_for_head,
                        "reply_to": user_msg_id.to_string(),
                    }
                }),
            )
            .with_actor("human/_user");
            let mut rx3 = dispatcher.dispatch(
                req,
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx3.recv().await;
        }

        let _ = self.store.set_active_thread(scope.as_str(), user_msg_id);

        // Create a need for NeedService to dispatch to a head.
        // Use the chat scope so the head can respond in-thread.
        {
            let Some(k) = Kernel::get() else {
                return Box::pin(tokio_stream::once(ChatChunk::Error(
                    "Kernel not initialized".to_string(),
                )));
            };

            let req = Frame::req(
                "need:enqueue",
                serde_json::json!({
                    "need_id": need_id,
                    "source": "user",
                    "priority": "normal",
                    "need": message_for_head,
                    "context": "",
                    "scope": scope.as_str(),
                    "reply_to": user_msg_id.to_string(),
                    "reconvene": false,
                }),
            )
            .with_actor("human/_user");

            let dispatcher = k.dispatcher().await;
            let mut rx2 = dispatcher.dispatch(
                req,
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx2.recv().await;
        }
        Box::pin(response_stream(rx))
    }
}

fn response_stream(
    rx: tokio::sync::mpsc::Receiver<Frame>,
) -> impl Stream<Item = ChatChunk> + Send + 'static {
    let s = ReceiverStream::new(rx);

    // Convert frames to chat chunks.
    let mapped = s.filter_map(|frame| match frame.op {
        FrameOp::Bytes => {
            let text = frame
                .data
                .as_ref()
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.is_empty() {
                return std::future::ready(None);
            }
            std::future::ready(Some(ChatChunk::Delta(text.to_string())))
        }
        FrameOp::Redirect => {
            let tool_call_id = frame
                .data
                .as_ref()
                .and_then(|v| v.get("tool_call_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = frame
                .data
                .as_ref()
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments_json = frame
                .data
                .as_ref()
                .and_then(|v| v.get("arguments"))
                .and_then(|v| v.as_str())
                .unwrap_or("{}")
                .to_string();
            if tool_call_id.is_empty() || name.is_empty() {
                return std::future::ready(Some(ChatChunk::Error(
                    "Malformed redirect".to_string(),
                )));
            }
            std::future::ready(Some(ChatChunk::ToolCall {
                tool_call_id,
                name,
                arguments_json,
            }))
        }
        FrameOp::Ok | FrameOp::Done => std::future::ready(Some(ChatChunk::Done)),
        FrameOp::Error => {
            let msg = frame
                .data
                .as_ref()
                .and_then(|v| v.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("Kernel error")
                .to_string();
            std::future::ready(Some(ChatChunk::Error(msg)))
        }
        _ => std::future::ready(None),
    });

    let timeout_stream = tokio_stream::StreamExt::timeout(mapped, Duration::from_secs(120));
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
