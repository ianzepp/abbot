//! Web Chat API - Simple HTTP+SSE chat endpoint for web UI
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module provides a minimal HTTP+SSE chat endpoint for web-based chat UIs.
//! Unlike the OpenAI-compatible endpoint, this is a simple request/response API:
//! - POST /api/chat with JSON { scope, text }
//! - Streams response via SSE events (delta, tool, done, error)
//!
//! Post-syscall-refactor, this adapter translates web chat requests into the same
//! internal syscall flow as the OpenAI adapter (chat:message, chat:tool_result, etc).
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Minimal protocol for web UIs (no OpenAI schema overhead)
//! - SSE streaming for real-time text deltas
//! - Cancellation on client disconnect via chat:cancel syscall
//! - No external tool support (internal tools only)

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{
    Json,
    extract::State,
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use futures::StreamExt;
use serde::Deserialize;
use tokio_stream::Stream;
use uuid::Uuid;

use crate::history::Store;
use crate::kernel::{Frame, FrameOp};
use crate::runtime::Kernel;

// =============================================================================
// TYPES
// =============================================================================

/// Axum state for web chat endpoint.
///
/// WHY: Simple state container holding only store (no ingress hub needed as
/// web chat dispatches syscalls directly).
#[derive(Clone)]
pub struct WebChatState {
    pub store: Arc<Store>,
}

impl WebChatState {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

/// Web chat request payload.
///
/// WHY: Minimal schema for web UI submissions (scope + text).
#[derive(Deserialize)]
pub struct WebChatRequest {
    pub scope: String,
    pub text: String,
}

// =============================================================================
// ENDPOINT
// =============================================================================

/// POST /api/chat - Submit message and stream response via SSE.
///
/// WHY: Provides a simple HTTP+SSE endpoint for web UIs without requiring
/// OpenAI protocol overhead.
pub async fn web_chat(
    State(state): State<WebChatState>,
    Json(req): Json<WebChatRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = handle_web_chat(state.store, req.scope, req.text).await;
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Handle web chat request: dispatch syscall and stream response.
///
/// WHY: Opens turn stream before dispatching chat:message syscall to prevent
/// race conditions. Returns SSE stream of deltas, tool calls, and done/error events.
async fn handle_web_chat(
    store: Arc<Store>,
    scope: String,
    text: String,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let Some(k) = Kernel::get() else {
        return futures::stream::once(async {
            Ok(Event::default().data("error: Kernel not initialized"))
        })
        .boxed();
    };

    if text.trim().is_empty() {
        return futures::stream::once(async { Ok(Event::default().data("error: Empty message")) })
            .boxed();
    }

    let user_msg_id = Uuid::new_v4();

    // WHY: Open turn stream BEFORE dispatching work to avoid race where head
    // output arrives before stream listener is ready.
    let rx = k.sigcalls().open(&scope, user_msg_id).await;

    let _ = store.set_active_thread(&scope, user_msg_id).await;

    // WHY: Dispatch chat:message syscall (actor="user") which logs and enqueues work.
    {
        let req = Frame::req(
            "chat:message",
            serde_json::json!({
                "scope": &scope,
                "reply_to": user_msg_id.to_string(),
                "content": &text,
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

    let finished = Arc::new(AtomicBool::new(false));
    let scope_for_cancel = scope.clone();
    let reply_to = user_msg_id;

    // WHY: Convert frame items to SSE events based on data.type.
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx).filter_map(|frame| async move {
        match frame.op {
            FrameOp::Item => {
                let data = frame.data.as_ref()?;
                match data.get("type").and_then(|v| v.as_str()) {
                    Some("text_delta") => {
                        let text = data
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if text.is_empty() {
                            None
                        } else {
                            Some(Ok(Event::default().event("delta").data(text.to_string())))
                        }
                    }
                    Some("tool_call") => Some(Ok(
                        Event::default().event("tool").data(
                            serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string()),
                        ),
                    )),
                    Some("done") => Some(Ok(Event::default().event("done").data(""))),
                    _ => None,
                }
            }
            FrameOp::Error => {
                let msg = frame
                    .data
                    .as_ref()
                    .and_then(|v| v.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");
                Some(Ok(Event::default().event("error").data(msg.to_string())))
            }
            _ => None,
        }
    });

    // WHY: Wrap stream to emit chat:cancel on drop if client disconnects.
    CancelOnDropStream {
        inner: Box::pin(stream),
        finished,
        scope: scope_for_cancel,
        reply_to,
    }
    .boxed()
}

// =============================================================================
// CANCELLATION ON DROP
// =============================================================================
//
// WHY: Client disconnects should trigger chat:cancel syscall so the head can
// observe cancellation and stop expensive operations (LLM calls, tool dispatch).

/// Stream wrapper that dispatches chat:cancel on drop if not finished.
///
/// WHY: Ensures heads can observe cancellation and clean up in-flight work.
struct CancelOnDropStream {
    inner: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>,
    finished: Arc<AtomicBool>,
    scope: String,
    reply_to: Uuid,
}

impl Stream for CancelOnDropStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let poll = self.inner.as_mut().poll_next(cx);
        if let std::task::Poll::Ready(None) = poll {
            self.finished.store(true, Ordering::SeqCst);
        }
        poll
    }
}

impl Drop for CancelOnDropStream {
    /// Dispatch chat:cancel syscall if stream dropped before completion.
    ///
    /// WHY: Allows head to observe cancellation and skip further LLM calls
    /// or internal tool dispatch, preventing wasted work.
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
