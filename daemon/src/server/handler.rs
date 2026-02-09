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

use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::history::Store;
use crate::kernel::{Frame, FrameOp};
use crate::server::runtime::ChatRuntime;

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
    pub room: Option<String>,
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
    runtime: Arc<dyn ChatRuntime>,
}

impl ChatHandler {
    pub fn new(store: Arc<Store>, _head_id: impl Into<String>) -> Self {
        Self::with_runtime(
            store,
            _head_id,
            Arc::new(crate::server::runtime::KernelChatRuntime::new()),
        )
    }

    pub fn with_runtime(
        store: Arc<Store>,
        _head_id: impl Into<String>,
        runtime: Arc<dyn ChatRuntime>,
    ) -> Self {
        Self { store, runtime }
    }

    /// Stream an existing turn (for tool result resumption).
    ///
    /// WHY: Opens a turn stream listener for an already-enqueued turn.
    /// Used when tool results resume an existing need rather than creating a new one.
    pub async fn stream_existing(
        &self,
        room: &str,
        thread_id: Uuid,
    ) -> BoxStream<'static, ChatChunk> {
        let rx = match self.runtime.open_stream(room, thread_id).await {
            Ok(rx) => rx,
            Err(e) => {
                return Box::pin(tokio_stream::once(ChatChunk::Error(e)));
            }
        };
        Box::pin(cancel_on_drop(
            room,
            thread_id,
            response_stream(rx),
            self.runtime.clone(),
        ))
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
            .and_then(|m| super::session_scope::extract_env_block(&m.content));

        if let Some(ref env) = env_block {
            tracing::debug!(
                env_len = env.len(),
                "extracted env block from system prompt"
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

        // WHY: Explicit /reset command strips the prefix before forwarding to head.
        let mut message_for_head = last_user_message;
        let trimmed = message_for_head.trim_start();
        if let Some(rest) = trimmed.strip_prefix("/reset") {
            message_for_head = rest.trim_start().to_string();
        }

        tracing::debug!(
            content_len = message_for_head.len(),
            "user message for head"
        );

        let room = request.room.as_deref().unwrap_or("main").to_string();

        if let Some(ref env) = env_block {
            let _ = self.store.set_room_env(&room, env).await;
        }

        let user_msg_id = Uuid::new_v4();

        // -------------------------------------------------------------------------
        // PHASE 2: OPEN TURN STREAM
        // Open the turn stream BEFORE dispatching work to ensure no output is lost.
        // -------------------------------------------------------------------------
        let rx = match self.runtime.open_stream(&room, user_msg_id).await {
            Ok(rx) => rx,
            Err(e) => {
                return Box::pin(tokio_stream::once(ChatChunk::Error(e)));
            }
        };

        let _ = self.store.set_active_thread(&room, user_msg_id).await;

        // -------------------------------------------------------------------------
        // PHASE 3: DISPATCH CHAT SYSCALL
        // Submit chat:message syscall (actor="user") which logs and enqueues work.
        // -------------------------------------------------------------------------
        {
            let req = Frame::req(
                "chat:message",
                serde_json::json!({
                    "room": room,
                    "reply_to": user_msg_id.to_string(),
                    "content": message_for_head,
                    "interactive": true,
                }),
            )
            .with_actor("user");

            let workspace =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            match self.runtime.dispatch(req, workspace).await {
                Ok(mut rx2) => {
                    let _ = rx2.recv().await;
                }
                Err(e) => {
                    return Box::pin(tokio_stream::once(ChatChunk::Error(e)));
                }
            }
        }
        Box::pin(cancel_on_drop(
            &room,
            user_msg_id,
            response_stream(rx),
            self.runtime.clone(),
        ))
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
    room: &str,
    reply_to: Uuid,
    stream: impl Stream<Item = ChatChunk> + Send + 'static,
    runtime: Arc<dyn ChatRuntime>,
) -> impl Stream<Item = ChatChunk> + Send + 'static {
    let finished = Arc::new(AtomicBool::new(false));
    let room = room.to_string();
    CancelOnDropStream {
        inner: Box::pin(stream),
        finished,
        room,
        reply_to,
        runtime,
    }
}

struct CancelOnDropStream {
    inner: Pin<Box<dyn Stream<Item = ChatChunk> + Send>>,
    finished: Arc<AtomicBool>,
    room: String,
    reply_to: Uuid,
    runtime: Arc<dyn ChatRuntime>,
}

impl Stream for CancelOnDropStream {
    type Item = ChatChunk;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let poll = self.inner.as_mut().poll_next(cx);
        if let std::task::Poll::Ready(Some(ref chunk)) = poll
            && matches!(chunk, ChatChunk::Done | ChatChunk::Error(_))
        {
            self.finished.store(true, Ordering::SeqCst);
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
        let room = self.room.clone();
        let reply_to = self.reply_to;
        let runtime = self.runtime.clone();
        tokio::spawn(async move {
            let req = Frame::req(
                "chat:cancel",
                serde_json::json!({
                    "room": room,
                    "reply_to": reply_to.to_string(),
                    "reason": "client_disconnect",
                }),
            )
            .with_actor("system");
            let workspace =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            if let Ok(mut rx) = runtime.dispatch(req, workspace).await {
                let _ = rx.recv().await;
            }
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
                    let text = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
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
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use tempfile::TempDir;
    use uuid::Uuid;

    use crate::history::Store;
    use crate::kernel::{Frame, FrameOp};

    #[test]
    fn test_extract_env_block() {
        let content = "hello <env>{\"a\":1}</env> world";
        let block = crate::server::session_scope::extract_env_block(content).unwrap();
        assert_eq!(block, "<env>{\"a\":1}</env>");

        assert!(crate::server::session_scope::extract_env_block("no env here").is_none());
        assert!(crate::server::session_scope::extract_env_block("<env>missing end").is_none());
    }

    #[tokio::test]
    async fn test_response_stream_text_delta() {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let stream = response_stream(rx);

        tx.send(Frame {
            id: Uuid::new_v4(),
            ts: 0,
            op: FrameOp::Item,
            name: None,
            parent_id: None,
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(serde_json::json!({ "type": "text_delta", "content": "hi" })),
        })
        .await
        .unwrap();
        drop(tx);

        futures::pin_mut!(stream);
        let first = stream.next().await.unwrap();
        match first {
            ChatChunk::Delta(s) => assert_eq!(s, "hi"),
            _ => panic!("expected Delta"),
        }
    }

    #[tokio::test]
    async fn test_response_stream_tool_call_malformed() {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let stream = response_stream(rx);

        tx.send(Frame {
            id: Uuid::new_v4(),
            ts: 0,
            op: FrameOp::Item,
            name: None,
            parent_id: None,
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(serde_json::json!({ "type": "tool_call", "name": "" })),
        })
        .await
        .unwrap();
        drop(tx);

        futures::pin_mut!(stream);
        let first = stream.next().await.unwrap();
        match first {
            ChatChunk::Error(msg) => assert!(msg.contains("Malformed tool call")),
            _ => panic!("expected Error"),
        }
    }

    #[tokio::test]
    async fn test_handle_chat_empty_user_message() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path().join("history.db")).await.unwrap();
        let handler = ChatHandler::new(std::sync::Arc::new(store), "head/test");

        let req = ChatRequest {
            messages: vec![],
            stream: true,
            room: None,
        };

        let mut stream = handler.handle_chat(req).await;
        let first = stream.next().await.unwrap();
        match first {
            ChatChunk::Error(msg) => assert!(msg.contains("No user message provided")),
            _ => panic!("expected Error"),
        }
    }
}
