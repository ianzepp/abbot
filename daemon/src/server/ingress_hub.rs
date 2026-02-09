//! Ingress Hub - Protocol-agnostic chat request orchestration
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! IngressHub coordinates user input submission and tool result delivery across
//! HTTP-based ingress protocols (OpenAI-compatible, web chat). It ensures the
//! turn stream exists before delivering work to the kernel, preventing race
//! conditions where head output might be emitted before a stream listener is ready.
//!
//! Post-syscall-refactor, this layer translates protocol requests into the canonical
//! `chat:message` and `chat:tool_result` syscalls defined in the spec. The ChatHandler
//! opens turn streams and dispatches syscalls; IngressHub handles room validation
//! and tool result correlation.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Protocol adapters submit work through IngressHub, never directly to kernel
//! - Turn streams are opened BEFORE work is dispatched (race prevention)
//! - Tool results resume the same need using `chat:tool_result` (no new need created)
//! - Room validation enforces boundaries consistently

use std::sync::Arc;

use futures::stream::BoxStream;

use crate::history::Store;
use crate::kernel::Frame;
use crate::runtime::Kernel;

use super::handler::{ChatChunk, ChatHandler};

// =============================================================================
// HELPERS
// =============================================================================

/// Validate room name format.
fn is_valid_room_name(room: &str) -> bool {
    !room.trim().is_empty()
}

// =============================================================================
// INGRESS HUB
// =============================================================================

/// Orchestrates ingress requests across protocols.
///
/// WHY: Single entry point for all protocol adapters ensures consistent
/// turn stream lifecycle management and eliminates race conditions where
/// output could be emitted before a listener is ready.
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

    /// Submit a new user message to the kernel.
    ///
    /// WHY: Opens turn stream before dispatching work to prevent race
    /// conditions. Returns immediately as a stream to support SSE/streaming.
    pub async fn submit_user_turn(
        &self,
        room: &str,
        request: super::handler::ChatRequest,
    ) -> BoxStream<'static, ChatChunk> {
        if !is_valid_room_name(room) {
            return Box::pin(tokio_stream::once(ChatChunk::Error(format!(
                "Invalid room name '{}'",
                room
            ))));
        }

        let mut req = request;
        req.room = Some(room.to_string());
        self.chat.handle_chat(req).await
    }

    /// Submit tool results for a prior external tool call.
    ///
    /// WHY: Resumes the existing need using `chat:tool_result` syscall rather
    /// than creating a new need. This allows multi-turn tool execution without
    /// closing the client connection or losing context.
    ///
    /// SECURITY NOTE: Tool results are correlated by `tool_call_id` to prevent
    /// injection of arbitrary results into unrelated turns.
    pub async fn submit_tool_results(
        &self,
        room: &str,
        tool_results: Vec<(String, String)>,
        _stream: bool,
    ) -> Result<BoxStream<'static, ChatChunk>, (axum::http::StatusCode, String)> {
        use axum::http::StatusCode;

        if !is_valid_room_name(room) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Invalid room name '{}'", room),
            ));
        }

        let Some(thread_id) = self.store.get_active_thread(room).await.ok().flatten() else {
            return Err((
                StatusCode::BAD_REQUEST,
                "Unsupported: no active thread for this room".to_string(),
            ));
        };

        let response_stream = self.chat.stream_existing(room, thread_id).await;

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

            let key = crate::kernel::TurnKey::new(room, thread_id);
            let tool_name = k
                .turns()
                .pending_tool_name(&key, &tool_call_id)
                .await
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Unknown tool_call_id '{tool_call_id}'"),
                    )
                })?;

            let req = Frame::req(
                "chat:tool_result",
                serde_json::json!({
                    "room": room,
                    "reply_to": thread_id.to_string(),
                    "tool_call_id": tool_call_id,
                    "name": tool_name,
                    "content": output,
                    "is_error": false,
                }),
            )
            .with_actor("user");

            let mut rx = dispatcher.dispatch(
                req,
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                tokio_util::sync::CancellationToken::new(),
            );

            if let Some(frame) = rx.recv().await
                && frame.op == crate::kernel::FrameOp::Error
            {
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

        Ok(response_stream)
    }
}
