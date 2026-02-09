// Anthropic-compatible API endpoint.
//
// Implements POST /v1/messages with SSE streaming.
// Supports tool_use / tool_result content blocks for Claude Code compatibility.
// See: https://docs.anthropic.com/en/api/messages

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::ConnectInfo;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tokio_stream::Stream;

use super::IngressHub;
use super::handler::{ChatChunk, ChatMessage, ChatRequest, Role};
use super::session_scope::{api_key_or_bearer, derive_room_name, extract_cwd_heuristic};
use super::user_prompt::process_user_system_prompt;
use crate::history::{Store, ToolRegistryTool};
use crate::runtime::Kernel;

// =============================================================================
// STATE
// =============================================================================

#[derive(Clone)]
pub struct AnthropicState {
    pub ingress: Arc<IngressHub>,
    pub store: Arc<Store>,
}

impl AnthropicState {
    pub fn new(store: Arc<Store>, head_id: &str) -> Self {
        Self {
            ingress: Arc::new(IngressHub::new(store.clone(), head_id)),
            store,
        }
    }
}

// =============================================================================
// ERROR HELPERS
// =============================================================================

fn anthropic_error(status: StatusCode, error_type: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(AnthropicError {
            error_type: "error".to_string(),
            error: AnthropicErrorDetail {
                error_type: error_type.to_string(),
                message: message.into(),
            },
        }),
    )
        .into_response()
}

// =============================================================================
// REQUEST TYPES
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AnthropicRequest {
    pub model: String,
    #[serde(default)]
    pub system: Option<AnthropicContent>,
    pub messages: Vec<AnthropicMessage>,
    pub max_tokens: u32,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub tools: Vec<AnthropicTool>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: AnthropicContent,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

#[derive(Debug, Deserialize)]
pub struct AnthropicContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    // text block
    #[serde(default)]
    pub text: Option<String>,
    // tool_use block
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    // tool_result block
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub content: Option<AnthropicToolResultContent>,
    #[serde(default)]
    pub is_error: Option<bool>,
}

/// Tool result content can be a string or array of content blocks.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AnthropicToolResultContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

impl std::fmt::Display for AnthropicToolResultContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnthropicToolResultContent::Text(s) => f.write_str(s),
            AnthropicToolResultContent::Blocks(blocks) => {
                let joined: String = blocks
                    .iter()
                    .filter_map(|b| b.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                f.write_str(&joined)
            }
        }
    }
}

impl std::fmt::Display for AnthropicContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnthropicContent::Text(s) => f.write_str(s),
            AnthropicContent::Blocks(blocks) => {
                let joined: String = blocks
                    .iter()
                    .filter_map(|b| b.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                f.write_str(&joined)
            }
        }
    }
}

// =============================================================================
// RESPONSE TYPES
// =============================================================================

#[derive(Debug, Serialize)]
pub struct AnthropicResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub response_type: String,
    pub role: String,
    pub content: Vec<serde_json::Value>,
    pub model: String,
    pub stop_reason: String,
    pub stop_sequence: Option<String>,
    pub usage: AnthropicUsage,
}

#[derive(Debug, Serialize)]
pub struct AnthropicUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct AnthropicError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub error: AnthropicErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct AnthropicErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

// =============================================================================
// HELPERS
// =============================================================================

fn system_text(req: &AnthropicRequest) -> String {
    req.system
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn message_id() -> String {
    format!("msg_{}", uuid::Uuid::new_v4().to_string().replace("-", ""))
}

fn convert_role(role: &str) -> Role {
    match role {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => Role::User,
    }
}

fn convert_request(req: &AnthropicRequest) -> ChatRequest {
    let mut messages = Vec::new();

    if let Some(system) = req.system.as_ref() {
        messages.push(ChatMessage {
            role: Role::System,
            content: system.to_string(),
        });
    }

    for m in &req.messages {
        messages.push(ChatMessage {
            role: convert_role(&m.role),
            content: m.content.to_string(),
        });
    }

    ChatRequest {
        messages,
        stream: req.stream,
        room: None,
    }
}

fn summarize_tool_description(s: &str) -> String {
    let one_line = s
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    let mut out = String::new();
    for ch in one_line.chars() {
        if ch.is_whitespace() {
            if out.ends_with(' ') {
                continue;
            }
            out.push(' ');
        } else {
            out.push(ch);
        }
        if out.len() >= 220 {
            break;
        }
    }
    out.trim().to_string()
}

/// Check if the last user message contains tool_result content blocks.
fn is_tool_result_submission(req: &AnthropicRequest) -> bool {
    let last_user = req.messages.iter().rev().find(|m| m.role == "user");
    let Some(msg) = last_user else {
        return false;
    };
    match &msg.content {
        AnthropicContent::Blocks(blocks) => blocks.iter().any(|b| b.block_type == "tool_result"),
        AnthropicContent::Text(_) => false,
    }
}

/// Extract (tool_use_id, content) pairs from trailing tool_result blocks
/// in the last user message.
fn extract_tool_results(req: &AnthropicRequest) -> Vec<(String, String)> {
    let last_user = req.messages.iter().rev().find(|m| m.role == "user");
    let Some(msg) = last_user else {
        return Vec::new();
    };
    match &msg.content {
        AnthropicContent::Blocks(blocks) => blocks
            .iter()
            .filter(|b| b.block_type == "tool_result")
            .filter_map(|b| {
                let tool_use_id = b.tool_use_id.as_ref()?.clone();
                let content = b
                    .content
                    .as_ref()
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                Some((tool_use_id, content))
            })
            .collect(),
        AnthropicContent::Text(_) => Vec::new(),
    }
}

#[allow(clippy::result_large_err)]
fn require_loopback(peer_addr: SocketAddr) -> Result<(), Response> {
    if peer_addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(anthropic_error(
            StatusCode::FORBIDDEN,
            "permission_error",
            "Abbot is loopback-only (non-loopback clients are not supported)",
        ))
    }
}

// =============================================================================
// HANDLER
// =============================================================================

pub async fn messages(
    State(state): State<AnthropicState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<AnthropicRequest>,
) -> Response {
    if let Err(r) = require_loopback(peer_addr) {
        return r;
    }
    tracing::debug!(
        model = %request.model,
        stream = %request.stream,
        message_count = %request.messages.len(),
        has_system = %request.system.is_some(),
        tool_count = %request.tools.len(),
        "incoming anthropic messages request"
    );

    // -------------------------------------------------------------------------
    // PHASE 1: ROOM DERIVATION
    // Accept any request that has a token (via x-api-key or Bearer).
    // Use cwd heuristic on system prompt for session scoping.
    // Localhost without token gets main room.
    // -------------------------------------------------------------------------

    let token = api_key_or_bearer(&headers);
    let sys = system_text(&request);
    let cwd = extract_cwd_heuristic(&sys);
    let is_localhost = peer_addr.ip().is_loopback();

    let room = match (token, &cwd) {
        (Some(tok), Some(cwd_str)) => {
            let s = derive_room_name(tok, cwd_str);
            tracing::info!(room = %s, client_cwd = %cwd_str, "anthropic session room derived");
            s
        }
        (Some(tok), None) => {
            // Token but no cwd — derive room from token alone
            let s = derive_room_name(tok, "unknown");
            tracing::info!(room = %s, "anthropic session room (no cwd)");
            s
        }
        (None, _) if is_localhost => {
            tracing::info!("localhost anthropic request to main room");
            "main".to_string()
        }
        (None, _) => {
            return anthropic_error(
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "Missing authentication: provide x-api-key header or Authorization: Bearer token",
            );
        }
    };

    // -------------------------------------------------------------------------
    // PHASE 2: SYSTEM PROMPT CACHING
    // -------------------------------------------------------------------------

    if let Some(system_block) = request.system.as_ref() {
        let tool_names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
        if let Err(err) = process_user_system_prompt(
            state.store.clone(),
            room.as_str(),
            &system_block.to_string(),
            &tool_names,
        )
        .await
        {
            tracing::warn!(room = %room, error = %err, "failed to cache user system prompt");
        }
    }

    // -------------------------------------------------------------------------
    // PHASE 3: TOOL REGISTRATION
    // Register client-provided tools (scoped to session).
    // Anthropic uses `input_schema` instead of `parameters`.
    // -------------------------------------------------------------------------

    let ext_tools: Vec<ToolRegistryTool> = request
        .tools
        .iter()
        .map(|t| {
            let desc = t.description.clone().unwrap_or_default();
            let summary = summarize_tool_description(&desc);
            ToolRegistryTool {
                name: t.name.clone(),
                summary: if summary.is_empty() {
                    format!("{} (external tool)", t.name)
                } else {
                    summary
                },
                description: desc,
                schema_json: t
                    .input_schema
                    .clone()
                    .unwrap_or(serde_json::Value::Null)
                    .to_string(),
            }
        })
        .collect();

    if !ext_tools.is_empty() {
        let tools_json: Vec<serde_json::Value> = ext_tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "summary": t.summary,
                    "description": t.description,
                    "schema_json": t.schema_json,
                })
            })
            .collect();

        let Some(k) = Kernel::get() else {
            return anthropic_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "Kernel not initialized",
            );
        };

        let dispatcher = k.dispatcher().await;
        let req = crate::kernel::Frame::req(
            "tool:register",
            serde_json::json!({
                "room": room,
                "tools": tools_json,
            }),
        )
        .with_actor("server/anthropic");

        let mut rx = dispatcher.dispatch(
            req,
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;

        tracing::info!(room = %room, tool_count = ext_tools.len(), "external tools registered (anthropic)");
    }

    // -------------------------------------------------------------------------
    // PHASE 4: TOOL RESULT SUBMISSION
    // If last user message contains tool_result blocks, resume existing need.
    // -------------------------------------------------------------------------

    if is_tool_result_submission(&request) {
        let tool_results = extract_tool_results(&request);

        if tool_results.is_empty() {
            return anthropic_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "tool_result blocks found but no valid tool_use_id + content pairs",
            );
        }

        let response_stream = match state
            .ingress
            .submit_tool_results(room.as_str(), tool_results, request.stream)
            .await
        {
            Ok(s) => s,
            Err((code, msg)) => return anthropic_error(code, "invalid_request_error", msg),
        };

        if request.stream {
            let sse_stream = to_sse_stream(response_stream, request.model.clone());
            return Sse::new(sse_stream)
                .keep_alive(KeepAlive::default())
                .into_response();
        }

        return collect_non_streaming(response_stream, request.model.clone()).await;
    }

    // -------------------------------------------------------------------------
    // PHASE 5: NEW USER MESSAGE
    // -------------------------------------------------------------------------

    let model = request.model.clone();
    let stream = request.stream;
    let mut chat_request = convert_request(&request);
    chat_request.room = Some(room.clone());

    if stream {
        let response_stream = state
            .ingress
            .submit_user_turn(room.as_str(), chat_request)
            .await;
        let sse_stream = to_sse_stream(response_stream, model);
        Sse::new(sse_stream)
            .keep_alive(KeepAlive::default())
            .into_response()
    } else {
        let response_stream = state
            .ingress
            .submit_user_turn(room.as_str(), chat_request)
            .await;
        collect_non_streaming(response_stream, model).await
    }
}

// =============================================================================
// NON-STREAMING RESPONSE
// =============================================================================

async fn collect_non_streaming(
    mut stream: impl Stream<Item = ChatChunk> + Send + Unpin + 'static,
    model: String,
) -> Response {
    let mut content_blocks: Vec<serde_json::Value> = Vec::new();
    let mut has_tool_use = false;
    let mut text_buf = String::new();

    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Delta(text) => text_buf.push_str(&text),
            ChatChunk::ToolCall {
                tool_call_id,
                name,
                arguments_json,
            } => {
                // Flush accumulated text as a text block
                if !text_buf.is_empty() {
                    content_blocks.push(serde_json::json!({
                        "type": "text",
                        "text": text_buf,
                    }));
                    text_buf.clear();
                }
                // Parse arguments_json back to Value for proper nesting
                let input: serde_json::Value =
                    serde_json::from_str(&arguments_json).unwrap_or(serde_json::json!({}));
                content_blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tool_call_id,
                    "name": name,
                    "input": input,
                }));
                has_tool_use = true;
            }
            ChatChunk::Done => break,
            ChatChunk::Error(e) => {
                return anthropic_error(StatusCode::INTERNAL_SERVER_ERROR, "api_error", e);
            }
        }
    }

    // Flush remaining text
    if !text_buf.is_empty() {
        content_blocks.push(serde_json::json!({
            "type": "text",
            "text": text_buf,
        }));
    }

    // Ensure at least one content block
    if content_blocks.is_empty() {
        content_blocks.push(serde_json::json!({
            "type": "text",
            "text": "",
        }));
    }

    let stop_reason = if has_tool_use { "tool_use" } else { "end_turn" };

    Json(AnthropicResponse {
        id: message_id(),
        response_type: "message".to_string(),
        role: "assistant".to_string(),
        content: content_blocks,
        model,
        stop_reason: stop_reason.to_string(),
        stop_sequence: None,
        usage: AnthropicUsage {
            input_tokens: 0,
            output_tokens: 0,
        },
    })
    .into_response()
}

// =============================================================================
// SSE STREAM CONVERSION
// =============================================================================

fn to_sse_stream(
    stream: impl Stream<Item = ChatChunk> + Send + 'static,
    model: String,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let msg_id = message_id();

    // Emit message_start as prefix
    let message_start_json = serde_json::json!({
        "type": "message_start",
        "message": {
            "id": msg_id,
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": model,
            "stop_reason": null,
            "stop_sequence": null,
            "usage": {"input_tokens": 0, "output_tokens": 0}
        }
    });

    let prefix = tokio_stream::iter(vec![Ok(Event::default()
        .event("message_start")
        .data(message_start_json.to_string()))]);

    // State: (block_index, has_tool_use, text_block_open)
    let content_stream = stream.scan((0u32, false, false), move |state, chunk| {
        let (block_index, has_tool_use, text_block_open) = state;

        let events: Vec<Event> = match chunk {
            ChatChunk::Delta(text) => {
                let mut evts = Vec::new();

                // Open a text block if not already open
                if !*text_block_open {
                    evts.push(
                        Event::default().event("content_block_start").data(
                            serde_json::json!({
                                "type": "content_block_start",
                                "index": *block_index,
                                "content_block": {"type": "text", "text": ""}
                            })
                            .to_string(),
                        ),
                    );
                    *text_block_open = true;
                }

                evts.push(
                    Event::default().event("content_block_delta").data(
                        serde_json::json!({
                            "type": "content_block_delta",
                            "index": *block_index,
                            "delta": {"type": "text_delta", "text": text}
                        })
                        .to_string(),
                    ),
                );

                evts
            }
            ChatChunk::ToolCall {
                tool_call_id,
                name,
                arguments_json,
            } => {
                let mut evts = Vec::new();

                // Close any open text block first
                if *text_block_open {
                    evts.push(
                        Event::default().event("content_block_stop").data(
                            serde_json::json!({
                                "type": "content_block_stop",
                                "index": *block_index
                            })
                            .to_string(),
                        ),
                    );
                    *block_index += 1;
                    *text_block_open = false;
                }

                // content_block_start for tool_use
                evts.push(
                    Event::default().event("content_block_start").data(
                        serde_json::json!({
                            "type": "content_block_start",
                            "index": *block_index,
                            "content_block": {
                                "type": "tool_use",
                                "id": tool_call_id,
                                "name": name,
                                "input": {}
                            }
                        })
                        .to_string(),
                    ),
                );

                // content_block_delta with input_json_delta
                evts.push(
                    Event::default().event("content_block_delta").data(
                        serde_json::json!({
                            "type": "content_block_delta",
                            "index": *block_index,
                            "delta": {
                                "type": "input_json_delta",
                                "partial_json": arguments_json
                            }
                        })
                        .to_string(),
                    ),
                );

                // content_block_stop
                evts.push(
                    Event::default().event("content_block_stop").data(
                        serde_json::json!({
                            "type": "content_block_stop",
                            "index": *block_index
                        })
                        .to_string(),
                    ),
                );

                *block_index += 1;
                *has_tool_use = true;

                evts
            }
            ChatChunk::Done => {
                let mut evts = Vec::new();

                // Close any open text block
                if *text_block_open {
                    evts.push(
                        Event::default().event("content_block_stop").data(
                            serde_json::json!({
                                "type": "content_block_stop",
                                "index": *block_index
                            })
                            .to_string(),
                        ),
                    );
                    *text_block_open = false;
                }

                let stop_reason = if *has_tool_use {
                    "tool_use"
                } else {
                    "end_turn"
                };

                evts.push(
                    Event::default().event("message_delta").data(
                        serde_json::json!({
                            "type": "message_delta",
                            "delta": {
                                "stop_reason": stop_reason,
                                "stop_sequence": null
                            },
                            "usage": {"output_tokens": 0}
                        })
                        .to_string(),
                    ),
                );

                evts.push(
                    Event::default()
                        .event("message_stop")
                        .data(serde_json::json!({"type": "message_stop"}).to_string()),
                );

                evts
            }
            ChatChunk::Error(e) => {
                vec![
                    Event::default().event("error").data(
                        serde_json::json!({
                            "type": "error",
                            "error": {
                                "type": "api_error",
                                "message": e
                            }
                        })
                        .to_string(),
                    ),
                ]
            }
        };

        std::future::ready(Some(tokio_stream::iter(
            events.into_iter().map(Ok::<_, Infallible>),
        )))
    });

    // Flatten the stream of streams
    let flat = content_stream.flatten();

    prefix.chain(flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request(messages_json: serde_json::Value) -> AnthropicRequest {
        serde_json::from_value(serde_json::json!({
            "model": "test-model",
            "max_tokens": 1024,
            "messages": messages_json,
        }))
        .unwrap()
    }

    // -- AnthropicContent deserialization --

    #[test]
    fn content_deserializes_from_string() {
        let c: AnthropicContent = serde_json::from_value(serde_json::json!("hello")).unwrap();
        assert_eq!(c.to_string(), "hello");
    }

    #[test]
    fn content_deserializes_from_text_blocks() {
        let c: AnthropicContent = serde_json::from_value(serde_json::json!([
            {"type": "text", "text": "hello "},
            {"type": "text", "text": "world"}
        ]))
        .unwrap();
        assert_eq!(c.to_string(), "hello \nworld");
    }

    // -- AnthropicTool deserialization --

    #[test]
    fn tool_deserializes_with_input_schema() {
        let t: AnthropicTool = serde_json::from_value(serde_json::json!({
            "name": "Bash",
            "description": "Run commands",
            "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}}
        }))
        .unwrap();
        assert_eq!(t.name, "Bash");
        assert!(t.input_schema.is_some());
    }

    #[test]
    fn tool_deserializes_without_optional_fields() {
        let t: AnthropicTool = serde_json::from_value(serde_json::json!({"name": "Read"})).unwrap();
        assert_eq!(t.name, "Read");
        assert!(t.description.is_none());
        assert!(t.input_schema.is_none());
    }

    // -- AnthropicContentBlock: tool_use --

    #[test]
    fn content_block_tool_use() {
        let b: AnthropicContentBlock = serde_json::from_value(serde_json::json!({
            "type": "tool_use",
            "id": "toolu_123",
            "name": "Bash",
            "input": {"command": "ls"}
        }))
        .unwrap();
        assert_eq!(b.block_type, "tool_use");
        assert_eq!(b.id.as_deref(), Some("toolu_123"));
        assert_eq!(b.name.as_deref(), Some("Bash"));
        assert!(b.input.is_some());
    }

    // -- AnthropicContentBlock: tool_result --

    #[test]
    fn content_block_tool_result_string() {
        let b: AnthropicContentBlock = serde_json::from_value(serde_json::json!({
            "type": "tool_result",
            "tool_use_id": "toolu_123",
            "content": "file1.txt\nfile2.txt"
        }))
        .unwrap();
        assert_eq!(b.block_type, "tool_result");
        assert_eq!(b.tool_use_id.as_deref(), Some("toolu_123"));
        assert_eq!(
            b.content.as_ref().unwrap().to_string(),
            "file1.txt\nfile2.txt"
        );
    }

    #[test]
    fn content_block_tool_result_blocks() {
        let b: AnthropicContentBlock = serde_json::from_value(serde_json::json!({
            "type": "tool_result",
            "tool_use_id": "toolu_456",
            "content": [{"type": "text", "text": "output line"}]
        }))
        .unwrap();
        assert_eq!(b.content.as_ref().unwrap().to_string(), "output line");
    }

    // -- AnthropicRequest: tools field --

    #[test]
    fn request_with_tools() {
        let req: AnthropicRequest = serde_json::from_value(serde_json::json!({
            "model": "claude-sonnet-4-5-20250929",
            "max_tokens": 1024,
            "tools": [
                {"name": "Bash", "description": "Run shell commands", "input_schema": {"type": "object"}},
                {"name": "Read", "description": "Read files"}
            ],
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .unwrap();
        assert_eq!(req.tools.len(), 2);
        assert_eq!(req.tools[0].name, "Bash");
    }

    #[test]
    fn request_without_tools_defaults_empty() {
        let req = make_request(serde_json::json!([{"role": "user", "content": "hi"}]));
        assert!(req.tools.is_empty());
    }

    // -- is_tool_result_submission --

    #[test]
    fn detects_tool_result_submission() {
        let req: AnthropicRequest = serde_json::from_value(serde_json::json!({
            "model": "test",
            "max_tokens": 1024,
            "messages": [
                {"role": "user", "content": "do something"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "file.txt"}
                ]}
            ]
        }))
        .unwrap();
        assert!(is_tool_result_submission(&req));
    }

    #[test]
    fn plain_text_not_tool_result() {
        let req = make_request(serde_json::json!([{"role": "user", "content": "hello"}]));
        assert!(!is_tool_result_submission(&req));
    }

    // -- extract_tool_results --

    #[test]
    fn extracts_tool_result_pairs() {
        let req: AnthropicRequest = serde_json::from_value(serde_json::json!({
            "model": "test",
            "max_tokens": 1024,
            "messages": [
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_a", "content": "output_a"},
                    {"type": "tool_result", "tool_use_id": "toolu_b", "content": "output_b"}
                ]}
            ]
        }))
        .unwrap();
        let results = extract_tool_results(&req);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], ("toolu_a".to_string(), "output_a".to_string()));
        assert_eq!(results[1], ("toolu_b".to_string(), "output_b".to_string()));
    }

    #[test]
    fn extract_tool_results_empty_for_text() {
        let req = make_request(serde_json::json!([{"role": "user", "content": "hello"}]));
        assert!(extract_tool_results(&req).is_empty());
    }

    // -- summarize_tool_description --

    #[test]
    fn summarize_collapses_whitespace() {
        let desc = "Run  shell\n  commands\n\n  safely";
        assert_eq!(
            summarize_tool_description(desc),
            "Run shell commands safely"
        );
    }

    #[test]
    fn summarize_truncates_at_220() {
        let long = "a ".repeat(200);
        let result = summarize_tool_description(&long);
        assert!(result.len() <= 221); // 220 + possible trailing char
    }

    // -- convert_request --

    #[test]
    fn convert_request_includes_system() {
        let req: AnthropicRequest = serde_json::from_value(serde_json::json!({
            "model": "test",
            "max_tokens": 1024,
            "system": "You are helpful",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .unwrap();
        let chat = convert_request(&req);
        assert_eq!(chat.messages.len(), 2);
        assert!(matches!(chat.messages[0].role, Role::System));
        assert!(matches!(chat.messages[1].role, Role::User));
    }

    #[test]
    fn convert_request_no_system() {
        let req = make_request(serde_json::json!([{"role": "user", "content": "hi"}]));
        let chat = convert_request(&req);
        assert_eq!(chat.messages.len(), 1);
        assert!(matches!(chat.messages[0].role, Role::User));
    }
}
