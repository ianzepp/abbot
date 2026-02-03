// HTTP endpoint for web UI chat submissions.
//
// Accepts POST /api/chat with { scope, text } and streams response via SSE.

use std::convert::Infallible;
use std::sync::Arc;

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

    let need_id = Uuid::new_v4().to_string();
    let user_msg_id = Uuid::new_v4();

    let rx = k.sigcalls().open(&scope, user_msg_id).await;

    {
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "log:append",
            serde_json::json!({
                "kind": "chat:user",
                "scope": &scope,
                "data": {
                    "content": &text,
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

    let _ = store.set_active_thread(&scope, user_msg_id);

    {
        let req = Frame::req(
            "need:enqueue",
            serde_json::json!({
                "need_id": need_id,
                "source": "web",
                "priority": "normal",
                "need": &text,
                "context": "",
                "scope": &scope,
                "reply_to": user_msg_id.to_string(),
                "reconvene": false,
            }),
        )
        .with_actor(format!("web/{}", scope));

        let dispatcher = k.dispatcher().await;
        let mut rx2 = dispatcher.dispatch(
            req,
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx2.recv().await;
    }

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    stream
        .filter_map(|frame| async move {
            match frame.op {
                FrameOp::Bytes => {
                    let text = frame
                        .data
                        .as_ref()
                        .and_then(|v| v.get("text"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if text.is_empty() {
                        None
                    } else {
                        Some(Ok(Event::default().event("delta").data(text.to_string())))
                    }
                }
                FrameOp::Redirect => {
                    let tool_name = frame
                        .data
                        .as_ref()
                        .and_then(|v| v.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool");
                    Some(Ok(Event::default()
                        .event("tool")
                        .data(format!("{{\"name\":\"{}\"}}", tool_name))))
                }
                FrameOp::Done | FrameOp::Ok => Some(Ok(Event::default().event("done").data(""))),
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
        })
        .boxed()
}
