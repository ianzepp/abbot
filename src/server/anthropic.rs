// Anthropic-compatible API endpoint.
//
// Implements POST /v1/messages with SSE streaming.
// See: https://docs.anthropic.com/en/api/messages

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio_stream::{Stream, StreamExt};

use super::handler::{ChatChunk, ChatHandler, ChatMessage, ChatRequest, Role};
use crate::history::Store;
use crate::runtime::RuntimeBus;

#[derive(Clone)]
pub struct AnthropicState {
    pub handler: Arc<ChatHandler>,
}

impl AnthropicState {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, head_id: &str) -> Self {
        Self {
            handler: Arc::new(ChatHandler::new(bus, store, head_id)),
        }
    }
}

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
    #[serde(default)]
    pub text: Option<String>,
}

impl AnthropicContent {
    fn to_string(&self) -> String {
        match self {
            AnthropicContent::Text(s) => s.clone(),
            AnthropicContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| b.text.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AnthropicResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub response_type: String,
    pub role: String,
    pub content: Vec<AnthropicResponseBlock>,
    pub model: String,
    pub stop_reason: String,
    pub stop_sequence: Option<String>,
    pub usage: AnthropicUsage,
}

#[derive(Debug, Serialize)]
pub struct AnthropicResponseBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
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

fn convert_role(role: &str) -> Role {
    match role {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => Role::User,
    }
}

fn convert_request(req: AnthropicRequest) -> ChatRequest {
    let mut messages = Vec::new();

    if let Some(system) = req.system {
        messages.push(ChatMessage {
            role: Role::System,
            content: system.to_string(),
        });
    }

    for m in req.messages {
        messages.push(ChatMessage {
            role: convert_role(&m.role),
            content: m.content.to_string(),
        });
    }

    ChatRequest {
        messages,
        stream: req.stream,
    }
}

fn message_id() -> String {
    format!("msg_{}", uuid::Uuid::new_v4().to_string().replace("-", ""))
}

fn stub_response(stream: bool, model: &str) -> Response {
    let content = "(stub)";

    if stream {
        let msg_id = message_id();
        let events = vec![
            Event::default()
                .event("message_start")
                .data(serde_json::json!({
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
                }).to_string()),
            Event::default()
                .event("content_block_start")
                .data(serde_json::json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""}
                }).to_string()),
            Event::default()
                .event("content_block_delta")
                .data(serde_json::json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": content}
                }).to_string()),
            Event::default()
                .event("content_block_stop")
                .data(serde_json::json!({"type": "content_block_stop", "index": 0}).to_string()),
            Event::default()
                .event("message_delta")
                .data(serde_json::json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                    "usage": {"output_tokens": 1}
                }).to_string()),
            Event::default()
                .event("message_stop")
                .data(serde_json::json!({"type": "message_stop"}).to_string()),
        ];

        let stream = tokio_stream::iter(events.into_iter().map(Ok::<_, Infallible>));
        Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
    } else {
        Json(AnthropicResponse {
            id: message_id(),
            response_type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![AnthropicResponseBlock {
                block_type: "text".to_string(),
                text: content.to_string(),
            }],
            model: model.to_string(),
            stop_reason: "end_turn".to_string(),
            stop_sequence: None,
            usage: AnthropicUsage {
                input_tokens: 0,
                output_tokens: 1,
            },
        }).into_response()
    }
}

pub async fn messages(
    State(state): State<AnthropicState>,
    Json(request): Json<AnthropicRequest>,
) -> Response {
    let is_haiku = request.model.contains("haiku");

    tracing::debug!(
        model = %request.model,
        stream = %request.stream,
        message_count = %request.messages.len(),
        has_system = %request.system.is_some(),
        is_haiku = is_haiku,
        "incoming anthropic messages request"
    );

    // Short-circuit haiku housekeeping requests (token counting, title generation, etc.)
    if is_haiku {
        tracing::debug!("short-circuiting haiku request with stub response");
        return stub_response(request.stream, &request.model);
    }

    let model = request.model.clone();
    let stream = request.stream;
    let chat_request = convert_request(request);

    if stream {
        let response_stream = state.handler.handle_chat(chat_request).await;
        let sse_stream = to_sse_stream(response_stream, model);
        Sse::new(sse_stream)
            .keep_alive(KeepAlive::default())
            .into_response()
    } else {
        let mut response_stream = state.handler.handle_chat(chat_request).await;
        let mut content = String::new();

        while let Some(chunk) = response_stream.next().await {
            match chunk {
                ChatChunk::Delta(text) => content.push_str(&text),
                ChatChunk::Done => break,
                ChatChunk::Error(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(AnthropicError {
                            error_type: "error".to_string(),
                            error: AnthropicErrorDetail {
                                error_type: "api_error".to_string(),
                                message: e,
                            },
                        }),
                    )
                        .into_response();
                }
            }
        }

        let response = AnthropicResponse {
            id: message_id(),
            response_type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![AnthropicResponseBlock {
                block_type: "text".to_string(),
                text: content,
            }],
            model,
            stop_reason: "end_turn".to_string(),
            stop_sequence: None,
            usage: AnthropicUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        };

        Json(response).into_response()
    }
}

#[derive(Debug, Serialize)]
struct StreamMessageStart {
    #[serde(rename = "type")]
    event_type: String,
    message: StreamMessage,
}

#[derive(Debug, Serialize)]
struct StreamMessage {
    id: String,
    #[serde(rename = "type")]
    msg_type: String,
    role: String,
    content: Vec<serde_json::Value>,
    model: String,
    stop_reason: Option<String>,
    stop_sequence: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Serialize)]
struct StreamContentBlockStart {
    #[serde(rename = "type")]
    event_type: String,
    index: u32,
    content_block: StreamContentBlock,
}

#[derive(Debug, Serialize)]
struct StreamContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct StreamContentBlockDelta {
    #[serde(rename = "type")]
    event_type: String,
    index: u32,
    delta: StreamTextDelta,
}

#[derive(Debug, Serialize)]
struct StreamTextDelta {
    #[serde(rename = "type")]
    delta_type: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct StreamContentBlockStop {
    #[serde(rename = "type")]
    event_type: String,
    index: u32,
}

#[derive(Debug, Serialize)]
struct StreamMessageDelta {
    #[serde(rename = "type")]
    event_type: String,
    delta: StreamMessageDeltaPayload,
    usage: StreamDeltaUsage,
}

#[derive(Debug, Serialize)]
struct StreamMessageDeltaPayload {
    stop_reason: String,
    stop_sequence: Option<String>,
}

#[derive(Debug, Serialize)]
struct StreamDeltaUsage {
    output_tokens: u32,
}

#[derive(Debug, Serialize)]
struct StreamMessageStop {
    #[serde(rename = "type")]
    event_type: String,
}

fn to_sse_stream(
    stream: impl Stream<Item = ChatChunk> + Send + 'static,
    model: String,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let msg_id = message_id();

    let message_start = StreamMessageStart {
        event_type: "message_start".to_string(),
        message: StreamMessage {
            id: msg_id.clone(),
            msg_type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![],
            model: model.clone(),
            stop_reason: None,
            stop_sequence: None,
            usage: AnthropicUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        },
    };

    let content_block_start = StreamContentBlockStart {
        event_type: "content_block_start".to_string(),
        index: 0,
        content_block: StreamContentBlock {
            block_type: "text".to_string(),
            text: String::new(),
        },
    };

    let prefix = tokio_stream::iter(vec![
        Ok(Event::default()
            .event("message_start")
            .data(serde_json::to_string(&message_start).unwrap())),
        Ok(Event::default()
            .event("content_block_start")
            .data(serde_json::to_string(&content_block_start).unwrap())),
    ]);

    let content_stream = stream.map(move |chunk| {
        match chunk {
            ChatChunk::Delta(text) => {
                let delta = StreamContentBlockDelta {
                    event_type: "content_block_delta".to_string(),
                    index: 0,
                    delta: StreamTextDelta {
                        delta_type: "text_delta".to_string(),
                        text,
                    },
                };
                Ok(Event::default()
                    .event("content_block_delta")
                    .data(serde_json::to_string(&delta).unwrap()))
            }
            ChatChunk::Done => {
                Ok(Event::default()
                    .event("content_block_stop")
                    .data(serde_json::to_string(&StreamContentBlockStop {
                        event_type: "content_block_stop".to_string(),
                        index: 0,
                    }).unwrap()))
            }
            ChatChunk::Error(e) => {
                Ok(Event::default()
                    .event("error")
                    .data(format!(r#"{{"type":"error","error":{{"type":"api_error","message":"{}"}}}}"#, e)))
            }
        }
    });

    let suffix = tokio_stream::iter(vec![
        Ok(Event::default()
            .event("message_delta")
            .data(serde_json::to_string(&StreamMessageDelta {
                event_type: "message_delta".to_string(),
                delta: StreamMessageDeltaPayload {
                    stop_reason: "end_turn".to_string(),
                    stop_sequence: None,
                },
                usage: StreamDeltaUsage { output_tokens: 0 },
            }).unwrap())),
        Ok(Event::default()
            .event("message_stop")
            .data(serde_json::to_string(&StreamMessageStop {
                event_type: "message_stop".to_string(),
            }).unwrap())),
    ]);

    prefix.chain(content_stream).chain(suffix)
}
