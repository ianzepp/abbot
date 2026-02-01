// OpenAI-compatible API endpoint.
//
// Implements GET /v1/models and POST /v1/chat/completions with SSE streaming.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use tokio_stream::{Stream, StreamExt};

use super::handler::{ChatChunk, ChatHandler, ChatMessage, ChatRequest, Role};
use crate::history::{Store, ToolRegistryTool};
use crate::runtime::RuntimeBus;

const MODEL_ID: &str = "abbot/default";

fn openai_error(status: StatusCode, message: impl Into<String>) -> Response {
    let message = message.into();
    (
        status,
        Json(serde_json::json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": "unsupported_client"
            }
        })),
    )
        .into_response()
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

fn contains_opencode_marker(req: &OpenAIChatRequest) -> bool {
    req.messages.iter().any(|m| {
        if m.role != "system" {
            return false;
        }
        let head = m
            .content
            .lines()
            .take(20)
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();
        head.contains("you are opencode")
    })
}

fn extract_env_block_from_system(req: &OpenAIChatRequest) -> Option<String> {
    let system = req.messages.iter().find(|m| m.role == "system")?;
    let content = system.content.as_str();
    let start = content.find("<env>")?;
    let end = content.find("</env>")?;
    if end <= start {
        return None;
    }
    Some(content[start..end + 6].to_string())
}

fn extract_env_cwd(env_block: &str) -> Option<String> {
    for line in env_block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Working directory:") {
            let cwd = rest.trim();
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

fn jwt_principal(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload_b64 = parts[1];
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    for key in ["sub", "email", "name"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn sha256_hex(s: &str) -> String {
    let mut h = sha2::Sha256::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    let mut hex = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", b);
    }
    hex
}

fn session_scope_from(token: &str, cwd: &str) -> String {
    let principal = jwt_principal(token)
        .unwrap_or_else(|| format!("token:{}", &sha256_hex(token)[..16]));
    let digest = sha256_hex(&format!("{}:{}", principal, cwd));
    format!("session/{}", &digest[..32])
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

fn log_headers(endpoint: &str, headers: &HeaderMap) {
    tracing::info!(endpoint, header_count = headers.len(), "http request headers");
    for (name, value) in headers.iter() {
        let key = name.as_str();
        if key == "authorization" {
            let Ok(v) = value.to_str() else {
                tracing::info!(endpoint, header = key, value = "(non-utf8)");
                continue;
            };
            let v = v.trim();
            if let Some(token) = v.strip_prefix("Bearer ") {
                let token = token.trim();
                let mut h = DefaultHasher::new();
                token.hash(&mut h);
                let digest = h.finish();
                tracing::info!(
                    endpoint,
                    header = key,
                    value = %format!("Bearer siphash64:{:016x} (len={})", digest, token.len())
                );
            } else {
                tracing::info!(endpoint, header = key, value = "(redacted)");
            }
            continue;
        }

        if matches!(key, "cookie" | "set-cookie") {
            tracing::info!(endpoint, header = key, value = "(redacted)");
            continue;
        }

        match value.to_str() {
            Ok(v) => tracing::info!(endpoint, header = key, value = v),
            Err(_) => tracing::info!(endpoint, header = key, value = "(non-utf8)"),
        }
    }
}

#[derive(Clone)]
pub struct OpenAIState {
    pub handler: Arc<ChatHandler>,
    pub store: Arc<Store>,
}

impl OpenAIState {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, head_id: &str) -> Self {
        Self {
            handler: Arc::new(ChatHandler::new(bus, store.clone(), head_id)),
            store,
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

fn convert_request(req: OpenAIChatRequest, scope: Option<String>) -> ChatRequest {
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
        scope,
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

pub async fn list_models(headers: HeaderMap) -> Json<OpenAIModelsResponse> {
    log_headers("GET /v1/models", &headers);
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
    headers: HeaderMap,
    Json(request): Json<OpenAIChatRequest>,
) -> Response {
    log_headers("POST /v1/chat/completions", &headers);
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

    let mut scope_override: Option<String> = None;

    // Strict, gated OpenCode compatibility mode.
    // Only activates if the system prompt explicitly identifies OpenCode.
    if contains_opencode_marker(&request) {
        let Some(token) = bearer_token(&headers) else {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "Unsupported: Opencode support requires authorization and environment information",
            );
        };
        let Some(env_block) = extract_env_block_from_system(&request) else {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "Unsupported: Opencode support requires authorization and environment information",
            );
        };
        let Some(cwd) = extract_env_cwd(&env_block) else {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "Unsupported: Opencode support requires a Working directory in the <env> block",
            );
        };

        let session_scope = session_scope_from(token, &cwd);
        tracing::info!(scope = %session_scope, client_cwd = %cwd, "opencode session scope derived");

        // Persist the external toolset for this session scope so the head can discover them.
        let ext_tools: Vec<ToolRegistryTool> = request
            .tools
            .iter()
            .filter(|t| t.tool_type == "function")
            .map(|t| {
                let desc = t.function.description.clone().unwrap_or_default();
                let summary = summarize_tool_description(&desc);
                ToolRegistryTool {
                    name: t.function.name.clone(),
                    summary: if summary.is_empty() {
                        format!("{} (external tool)", t.function.name)
                    } else {
                        summary
                    },
                    description: desc,
                    schema_json: t
                        .function
                        .parameters
                        .clone()
                        .unwrap_or(serde_json::Value::Null)
                        .to_string(),
                }
            })
            .collect();

        if let Err(e) = state.store.replace_external_tools(&session_scope, &ext_tools) {
            tracing::warn!(error = %e, scope = %session_scope, "failed to persist external tool registry");
        } else {
            tracing::info!(scope = %session_scope, tool_count = ext_tools.len(), "external tools registered");
        }

        scope_override = Some(session_scope);
    }

    let model = request.model.clone();
    let stream = request.stream;
    let chat_request = convert_request(request, scope_override);

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
