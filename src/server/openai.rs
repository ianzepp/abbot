// OpenAI-compatible API endpoint.
//
// Implements GET /v1/models and POST /v1/chat/completions with SSE streaming.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio_stream::{Stream, StreamExt};

use super::handler::{ChatChunk, ChatHandler, ChatMessage, ChatRequest, Role};
use crate::history::Store;
use crate::runtime::RuntimeBus;

const MODEL_ID: &str = "abbot/default";

#[derive(Clone)]
pub struct OpenAIState {
    pub handler: Arc<ChatHandler>,
}

impl OpenAIState {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, head_id: &str) -> Self {
        Self {
            handler: Arc::new(ChatHandler::new(bus, store, head_id)),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenAIChatRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Vec<OpenAITool>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAITool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunction,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIFunction {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAIChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIChoice>,
    pub usage: OpenAIUsage,
}

#[derive(Debug, Serialize)]
pub struct OpenAIChoice {
    pub index: u32,
    pub message: OpenAIResponseMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAIResponseMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// Models endpoint types
#[derive(Debug, Serialize)]
pub struct OpenAIModelsResponse {
    pub object: String,
    pub data: Vec<OpenAIModel>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIModel {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAIStreamChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIStreamChoice>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIStreamChoice {
    pub index: u32,
    pub delta: OpenAIDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

fn convert_role(role: &str) -> Role {
    match role {
        "system" => Role::System,
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => Role::User,
    }
}

fn convert_request(req: OpenAIChatRequest) -> ChatRequest {
    ChatRequest {
        messages: req
            .messages
            .into_iter()
            .map(|m| ChatMessage {
                role: convert_role(&m.role),
                content: m.content,
            })
            .collect(),
        stream: req.stream,
    }
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn response_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4().to_string().replace("-", "")[..24].to_string())
}

pub async fn list_models() -> Json<OpenAIModelsResponse> {
    Json(OpenAIModelsResponse {
        object: "list".to_string(),
        data: vec![OpenAIModel {
            id: MODEL_ID.to_string(),
            object: "model".to_string(),
            created: timestamp(),
            owned_by: "abbot".to_string(),
        }],
    })
}

pub async fn chat_completions(
    State(state): State<OpenAIState>,
    Json(request): Json<OpenAIChatRequest>,
) -> Response {
    // Debug: log incoming request from OpenCode
    tracing::debug!(
        model = %request.model,
        stream = %request.stream,
        message_count = %request.messages.len(),
        tool_count = %request.tools.len(),
        "incoming chat completion request"
    );

    for (i, msg) in request.messages.iter().enumerate() {
        if msg.role == "system" {
            tracing::info!(
                index = %i,
                role = %msg.role,
                content_len = %msg.content.len(),
                "system message from client:\n{}", msg.content
            );
        } else {
            tracing::debug!(
                index = %i,
                role = %msg.role,
                content_preview = %msg.content.chars().take(100).collect::<String>(),
                "message from client"
            );
        }
    }

    if !request.tools.is_empty() {
        tracing::info!(
            tool_count = %request.tools.len(),
            "tools from client:"
        );
        for tool in &request.tools {
            tracing::info!(
                name = %tool.function.name,
                description = %tool.function.description.as_deref().unwrap_or("(none)"),
                "  tool: {}", tool.function.name
            );
        }
    }

    let model = request.model.clone();
    let stream = request.stream;
    let chat_request = convert_request(request);

    if stream {
        let response_stream = state.handler.handle_chat(chat_request).await;
        let sse_stream = to_sse_stream(response_stream, model);
        Sse::new(sse_stream).keep_alive(KeepAlive::default()).into_response()
    } else {
        let mut response_stream = state.handler.handle_chat(chat_request).await;
        let mut content = String::new();

        while let Some(chunk) = response_stream.next().await {
            match chunk {
                ChatChunk::Delta(text) => content.push_str(&text),
                ChatChunk::Done => break,
                ChatChunk::Error(e) => {
                    return Json(serde_json::json!({
                        "error": {"message": e, "type": "server_error"}
                    }))
                    .into_response();
                }
            }
        }

        let response = OpenAIChatResponse {
            id: response_id(),
            object: "chat.completion".to_string(),
            created: timestamp(),
            model,
            choices: vec![OpenAIChoice {
                index: 0,
                message: OpenAIResponseMessage {
                    role: "assistant".to_string(),
                    content,
                },
                finish_reason: "stop".to_string(),
            }],
            usage: OpenAIUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        };

        Json(response).into_response()
    }
}

fn to_sse_stream(
    stream: impl Stream<Item = ChatChunk> + Send + 'static,
    model: String,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let id = response_id();
    let created = timestamp();
    let sent_role = false;

    stream.map(move |chunk| {
        let event = match chunk {
            ChatChunk::Delta(content) => {
                let delta = if !sent_role {
                    OpenAIDelta {
                        role: Some("assistant".to_string()),
                        content: Some(content),
                    }
                } else {
                    OpenAIDelta {
                        role: None,
                        content: Some(content),
                    }
                };

                let chunk = OpenAIStreamChunk {
                    id: id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created,
                    model: model.clone(),
                    choices: vec![OpenAIStreamChoice {
                        index: 0,
                        delta,
                        finish_reason: None,
                    }],
                };

                Event::default().data(serde_json::to_string(&chunk).unwrap())
            }
            ChatChunk::Done => {
                let chunk = OpenAIStreamChunk {
                    id: id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created,
                    model: model.clone(),
                    choices: vec![OpenAIStreamChoice {
                        index: 0,
                        delta: OpenAIDelta {
                            role: None,
                            content: None,
                        },
                        finish_reason: Some("stop".to_string()),
                    }],
                };

                Event::default().data(serde_json::to_string(&chunk).unwrap())
            }
            ChatChunk::Error(e) => Event::default().data(format!("{{\"error\": \"{}\"}}", e)),
        };

        Ok(event)
    })
    .chain(tokio_stream::once(Ok(Event::default().data("[DONE]"))))
}
