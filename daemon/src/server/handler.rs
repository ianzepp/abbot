//! Chat Handler - Protocol-agnostic chat completion backend
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! ChatHandler is the shared backend for all ingress protocols (OpenAI-compatible,
//! web chat). It translates protocol-specific requests into the canonical syscall
//! flow defined in the syscall refactor spec:
//!
//! - User input -> `chat:message` syscall (actor="user")
//! - Tool results -> `chat:tool_result` syscall (resume same need)
//! - Turn stream -> frame stream consumed by protocol adapters
//!
//! Post-syscall-refactor, this layer no longer directly calls kernel APIs or
//! manipulates turn state. Instead, it dispatches syscalls and consumes the turn
//! stream, converting frame items into protocol-agnostic ChatChunk enums.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Protocol adapters call ChatHandler, not kernel directly
//! - Turn stream is opened BEFORE work dispatch (race prevention)
//! - Stream cancellation triggers `chat:cancel` syscall (cleanup on disconnect)
//! - Single-message heuristic creates checkpoints for fresh sessions

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

// =============================================================================
// TYPES
// =============================================================================

/// Message role in protocol-agnostic representation.
///
/// WHY: Abstracts role semantics across OpenAI, Anthropic, and web chat protocols.
#[derive(Debug, Clone)]
pub enum Role {
    System,
    User,
    Assistant,
}

/// Protocol-agnostic chat message.
///
/// WHY: Provides a single representation for messages across all ingress protocols.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// Protocol-agnostic chat request.
///
/// WHY: Decouples ingress protocols from internal syscall representations.
#[derive(Debug)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
    pub scope: Option<String>,
}

/// Protocol-agnostic streaming response chunk.
///
/// WHY: Provides a single enum for protocol adapters to consume, regardless
/// of whether output is text, tool calls, completion signals, or errors.
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

// =============================================================================
// CHAT HANDLER
// =============================================================================

/// Shared chat completion handler for all ingress protocols.
///
/// WHY: Centralizes syscall dispatch and turn stream management so protocol
/// adapters don't duplicate logic or accidentally introduce race conditions.
pub struct ChatHandler {
    store: Arc<Store>,
}

impl ChatHandler {
    pub fn new(store: Arc<Store>, _head_id: impl Into<String>) -> Self {
        Self { store }
    }

    /// Stream an existing turn (for tool result resumption).
    ///
    /// WHY: Opens a turn stream listener for an already-enqueued turn.
    /// Used when tool results resume an existing need rather than creating a new one.
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

    /// Handle a new chat turn (user message submission).
    ///
    /// WHY: Opens turn stream before dispatching `chat:message` syscall to prevent
    /// race conditions where head output could arrive before the stream listener is ready.
    ///
    /// DESIGN TRADE-OFF: Single-message heuristic creates checkpoints automatically
    /// when a client sends only one user message. This improves UX for fresh sessions
    /// but could be surprising if clients expect full history to always be preserved.
    pub async fn handle_chat(&self, request: ChatRequest) -> BoxStream<'static, ChatChunk> {
        // -------------------------------------------------------------------------
        // PHASE 1: EXTRACT SESSION METADATA
        // Extract environment block and determine reset behavior based on message
        // history. The env block is persisted separately and injected into the head's
        // system prompt rather than passed through as a chat message.
        // -------------------------------------------------------------------------
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

        // WHY: Single-message heuristic creates a checkpoint when the client only sends
        // one user message (common in fresh sessions). This lets users start a new
        // conversation without explicitly deleting logs.
        let mut reset = false;
        if let Some((only,)) = non_system.as_slice().split_first().and_then(|(a, rest)| {
            if rest.is_empty() { Some((a,)) } else { None }
        }) {
            if matches!(only.role, Role::User) {
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

        // WHY: Explicit /reset command allows users to force a checkpoint mid-conversation.
        let mut message_for_head = last_user_message;
        let trimmed = message_for_head.trim_start();
        if let Some(rest) = trimmed.strip_prefix("/reset") {
            reset = true;
            message_for_head = rest.trim_start().to_string();
        }

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

        let user_msg_id = Uuid::new_v4();

        // -------------------------------------------------------------------------
        // PHASE 2: OPEN TURN STREAM
        // Open the turn stream BEFORE dispatching work to ensure no output is lost.
        // -------------------------------------------------------------------------
        let rx = k.sigcalls().open(scope.as_str(), user_msg_id).await;

        // WHY: Best-effort log of reset checkpoint for observability.
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

        // -------------------------------------------------------------------------
        // PHASE 3: DISPATCH CHAT SYSCALL
        // Submit chat:message syscall (actor="user") which logs and enqueues work.
        // -------------------------------------------------------------------------
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

// =============================================================================
// CANCELLATION ON DROP
// =============================================================================
//
// WHY: Client disconnects should trigger `chat:cancel` syscall to allow the head
// to clean up in-flight work. This wrapper ensures cancellation is issued when
// the stream is dropped before completion.

/// Wrap a stream to emit `chat:cancel` syscall on drop if not finished.
///
/// WHY: Prevents abandoned work from consuming resources. The head can observe
/// cancellation and skip further LLM calls or tool dispatch.
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
    /// Dispatch `chat:cancel` syscall if stream dropped before completion.
    ///
    /// WHY: Ensures head observes cancellation and can stop expensive operations
    /// like LLM calls or internal tool dispatch.
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

// =============================================================================
// FRAME TO CHUNK CONVERSION
// =============================================================================

/// Convert turn stream frames into protocol-agnostic ChatChunk enum.
///
/// WHY: Decouples protocol adapters from frame semantics. Post-syscall-refactor,
/// turn stream frames use `op=item` with `data.type` to indicate content type
/// (text_delta, tool_call, done). This function translates those into ChatChunk.
fn response_stream(
    rx: tokio::sync::mpsc::Receiver<Frame>,
) -> impl Stream<Item = ChatChunk> + Send + 'static {
    let s = ReceiverStream::new(rx);

    // WHY: Filter and map frames to ChatChunk based on op and data.type.
    // Only Item and Error ops are relevant to the turn stream.
    let mapped = s.filter_map(|frame| match frame.op {
        FrameOp::Item => {
            let Some(data) = frame.data.as_ref() else {
                return std::future::ready(None);
            };
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

    // WHY: 120-second timeout prevents hung streams from blocking client connections indefinitely.
    let timeout_stream = tokio_stream::StreamExt::timeout(mapped, Duration::from_secs(120));
    timeout_stream.map(|result| match result {
        Ok(chunk) => chunk,
        Err(_) => ChatChunk::Error("Response timeout".to_string()),
    })
}

// =============================================================================
// HELPERS
// =============================================================================

/// Extract <env>...</env> block from content.
///
/// WHY: Session environment is persisted separately and injected into the head's
/// system prompt rather than passed through as chat messages.
fn extract_env_block(content: &str) -> Option<String> {
    let start = content.find("<env>")?;
    let end = content.find("</env>")?;
    if end <= start {
        return None;
    }
    Some(content[start..end + 6].to_string())
}
