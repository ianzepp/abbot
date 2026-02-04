use std::sync::Arc;

use futures::stream::BoxStream;

use crate::Scope;
use crate::history::Store;
use crate::kernel::Frame;
use crate::runtime::Kernel;

use super::handler::{ChatChunk, ChatHandler};

fn is_valid_chat_scope(scope: &str) -> bool {
    let scope = scope.trim();
    scope == "main" || scope.starts_with("session/")
}

#[derive(Clone)]
pub struct IngressHub {
    store: Arc<Store>,
    chat: Arc<ChatHandler>,
}

impl IngressHub {
    pub fn new(store: Arc<Store>, head_id: &str) -> Self {
        Self {
            store: store.clone(),
            chat: Arc::new(ChatHandler::new(store, head_id)),
        }
    }

    pub async fn submit_user_turn(
        &self,
        scope: &str,
        request: super::handler::ChatRequest,
    ) -> BoxStream<'static, ChatChunk> {
        if !is_valid_chat_scope(scope) {
            return Box::pin(tokio_stream::once(ChatChunk::Error(
                format!("Unsupported scope '{}': must be 'main' or 'session/<hash>'", scope),
            )));
        }

        let mut req = request;
        req.scope = Some(scope.to_string());
        self.chat.handle_chat(req).await
    }

    pub async fn submit_tool_results(
        &self,
        scope: &str,
        tool_results: Vec<(String, String)>,
        _stream: bool,
    ) -> Result<BoxStream<'static, ChatChunk>, (axum::http::StatusCode, String)> {
        use axum::http::StatusCode;

        if !is_valid_chat_scope(scope) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Unsupported scope '{}': must be 'main' or 'session/<hash>'", scope),
            ));
        }

        let Some(thread_id) = self.store.get_active_thread(scope).ok().flatten() else {
            return Err((
                StatusCode::BAD_REQUEST,
                "Unsupported: no active thread for this session scope".to_string(),
            ));
        };

        // Open reply stream BEFORE delivering results to avoid races.
        let response_stream = self
            .chat
            .stream_existing(Scope::from(scope), thread_id)
            .await;

        // Deliver tool results into the kernel (resumes head processing).
        let Some(k) = Kernel::get() else {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Kernel not initialized".to_string(),
            ));
        };
        let dispatcher = k.dispatcher().await;

        for (tool_call_id, output) in tool_results {
            let tool_call_id = tool_call_id.trim().to_string();
            if tool_call_id.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Unsupported: tool messages must include tool_call_id".to_string(),
                ));
            }

            let req = Frame::req(
                "tool:result",
                serde_json::json!({
                    "scope": scope,
                    "tool_call_id": tool_call_id,
                    "output": output,
                }),
            )
            .with_actor("human/_user");

            let mut rx = dispatcher.dispatch(
                req,
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                tokio_util::sync::CancellationToken::new(),
            );

            if let Some(frame) = rx.recv().await {
                if frame.op == crate::kernel::FrameOp::Error {
                    let msg = frame
                        .data
                        .as_ref()
                        .and_then(|v| v.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unsupported tool result")
                        .to_string();
                    return Err((StatusCode::BAD_REQUEST, msg));
                }
            }
        }

        Ok(response_stream)
    }
}
