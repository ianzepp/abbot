// Shared handler for chat completions.
//
// Protocol-agnostic backend that:
// 1. Stores user message to SQLite
// 2. Publishes to head's scope
// 3. Wakes head immediately
// 4. Streams response chunks as head produces output

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::{StreamExt, Stream};
use futures::stream::BoxStream;
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
        Box::pin(cancel_on_drop(scope.as_str(), thread_id, response_stream(rx)))
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

        // Thread id for correlating reply stream + logging.
        let user_msg_id = Uuid::new_v4();

        // Open reply stream BEFORE publishing need to avoid races.
        let rx = k.sigcalls().open(scope.as_str(), user_msg_id).await;

        // Best-effort log of reset, if applicable.
        if reset {
            let dispatcher = k.dispatcher().await;
            let req = Frame::req(
                "log:append",
                serde_json::json!({
                    "kind": "chat:reset",
                    "scope": scope.as_str(),
                    "data": {"reason": "client_reset"}
                }),
            )
            .with_actor("user");
            let mut rx_reset = dispatcher.dispatch(
                req,
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx_reset.recv().await;
        }

        let _ = self.store.set_active_thread(scope.as_str(), user_msg_id);

        // Chat ingress (logs + need creation handled by chat:message).
        {
            let Some(k) = Kernel::get() else {
                return Box::pin(tokio_stream::once(ChatChunk::Error(
                    "Kernel not initialized".to_string(),
                )));
            };

            let req = Frame::req(
                "chat:message",
                serde_json::json!({
                    "scope": scope.as_str(),
                    "reply_to": user_msg_id.to_string(),
                    "content": message_for_head,
                }),
            )
            .with_actor("user");

            let dispatcher = k.dispatcher().await;
            let mut rx2 = dispatcher.dispatch(
                req,
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx2.recv().await;
        }
        Box::pin(cancel_on_drop(scope.as_str(), user_msg_id, response_stream(rx)))
    }
}

fn cancel_on_drop(
    scope: &str,
    reply_to: Uuid,
    stream: impl Stream<Item = ChatChunk> + Send + 'static,
) -> impl Stream<Item = ChatChunk> + Send + 'static {
    let finished = Arc::new(AtomicBool::new(false));
    let scope = scope.to_string();
    CancelOnDropStream {
        inner: Box::pin(stream),
        finished,
        scope,
        reply_to,
    }
}

struct CancelOnDropStream {
    inner: Pin<Box<dyn Stream<Item = ChatChunk> + Send>>,
    finished: Arc<AtomicBool>,
    scope: String,
    reply_to: Uuid,
}

impl Stream for CancelOnDropStream {
    type Item = ChatChunk;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let poll = self.inner.as_mut().poll_next(cx);
        if let std::task::Poll::Ready(Some(ref chunk)) = poll {
            if matches!(chunk, ChatChunk::Done | ChatChunk::Error(_)) {
                self.finished.store(true, Ordering::SeqCst);
            }
        }
        if let std::task::Poll::Ready(None) = poll {
            self.finished.store(true, Ordering::SeqCst);
        }
        poll
    }
}

impl Drop for CancelOnDropStream {
    fn drop(&mut self) {
        if self.finished.load(Ordering::SeqCst) {
            return;
        }
        let scope = self.scope.clone();
        let reply_to = self.reply_to;
        tokio::spawn(async move {
            let Some(k) = Kernel::get() else {
                return;
            };
            let dispatcher = k.dispatcher().await;
            let req = Frame::req(
                "chat:cancel",
                serde_json::json!({
                    "scope": scope,
                    "reply_to": reply_to.to_string(),
                    "reason": "client_disconnect",
                }),
            )
            .with_actor("system");
            let mut rx = dispatcher.dispatch(
                req,
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx.recv().await;
        });
    }
}

fn response_stream(
    rx: tokio::sync::mpsc::Receiver<Frame>,
) -> impl Stream<Item = ChatChunk> + Send + 'static {
    let s = ReceiverStream::new(rx);

    // Convert frames to chat chunks.
    let mapped = s.filter_map(|frame| match frame.op {
        FrameOp::Item => {
            let data = frame.data.as_ref()?;
            match data.get("type").and_then(|v| v.as_str()) {
                Some("text_delta") => {
                    let text = data
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if text.is_empty() {
                        return std::future::ready(None);
                    }
                    std::future::ready(Some(ChatChunk::Delta(text.to_string())))
                }
                Some("tool_call") => {
                    let tool_call_id = data
                        .get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = data
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if tool_call_id.is_empty() || name.is_empty() {
                        return std::future::ready(Some(ChatChunk::Error(
                            "Malformed tool call".to_string(),
                        )));
                    }
                    let arguments_json = data
                        .get("arguments")
                        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()))
                        .unwrap_or_else(|| "{}".to_string());
                    std::future::ready(Some(ChatChunk::ToolCall {
                        tool_call_id,
                        name,
                        arguments_json,
                    }))
                }
                Some("done") => std::future::ready(Some(ChatChunk::Done)),
                _ => std::future::ready(None),
            }
        }
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
