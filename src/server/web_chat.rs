// HTTP endpoint for web UI chat submissions.
//
// Accepts POST /api/chat with { scope, text } and streams response via SSE.

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

#[derive(Clone)]
pub struct WebChatState {
    pub store: Arc<Store>,
}

impl WebChatState {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
pub struct WebChatRequest {
    pub scope: String,
    pub text: String,
}

pub async fn web_chat(
    State(state): State<WebChatState>,
    Json(req): Json<WebChatRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = handle_web_chat(state.store, req.scope, req.text).await;
    Sse::new(stream).keep_alive(KeepAlive::default())
}

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

    let rx = k.sigcalls().open(&scope, user_msg_id).await;

    let _ = store.set_active_thread(&scope, user_msg_id);

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

    CancelOnDropStream {
        inner: Box::pin(stream),
        finished,
        scope: scope_for_cancel,
        reply_to,
    }
    .boxed()
}

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
