// OpenAI-compatible API endpoint.
//
// Implements GET /v1/models and POST /v1/chat/completions with SSE streaming.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use tokio_stream::Stream;

use super::handler::{ChatChunk, ChatHandler, ChatMessage, ChatRequest, Role};
use crate::bus::Scope;
use crate::history::{Store, ToolRegistryTool};
use crate::runtime::Kernel;

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
            .as_deref()
            .unwrap_or("")
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
    let content = system.content.as_deref()?;
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
    let principal =
        jwt_principal(token).unwrap_or_else(|| format!("token:{}", &sha256_hex(token)[..16]));
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
    tracing::info!(
        endpoint,
        header_count = headers.len(),
        "http request headers"
    );
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
    pub fn new(store: Arc<Store>, head_id: &str) -> Self {
        Self {
            handler: Arc::new(ChatHandler::new(store.clone(), head_id)),
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
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: OpenAIToolCallFunction,
}

#[derive(Debug, Serialize)]
pub struct OpenAIToolCallFunction {
    pub name: String,
    pub arguments: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIStreamToolCallDelta>>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIStreamToolCallDelta {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<OpenAIToolCallFunction>,
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
                content: m.content.unwrap_or_default(),
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
    format!(
        "chatcmpl-{}",
        uuid::Uuid::new_v4().to_string().replace("-", "")[..24].to_string()
    )
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
            let content = msg.content.as_deref().unwrap_or("");
            tracing::debug!(
                index = %i,
                role = %msg.role,
                content_len = %content.len(),
                "system message from client:\n{}",
                content
            );
        } else {
            let content = msg.content.as_deref().unwrap_or("");
            tracing::debug!(
                index = %i,
                role = %msg.role,
                content_preview = %content.chars().take(100).collect::<String>(),
                "message from client"
            );
        }
    }

    if !request.tools.is_empty() {
        tracing::debug!(
            tool_count = %request.tools.len(),
            "tools from client:"
        );
        for tool in &request.tools {
            tracing::debug!(
                name = %tool.function.name,
                description = %tool.function.description.as_deref().unwrap_or("(none)"),
                "  tool: {}", tool.function.name
            );
        }
    }

    let mut scope_override: Option<String> = None;

    // If this is a tool-result continuation turn (OpenCode), we don't create a new need.
    let has_tool_results = request.messages.iter().any(|m| m.role == "tool");

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

        if let Err(e) = state
            .store
            .replace_external_tools(&session_scope, &ext_tools)
        {
            tracing::warn!(error = %e, scope = %session_scope, "failed to persist external tool registry");
        } else {
            tracing::info!(scope = %session_scope, tool_count = ext_tools.len(), "external tools registered");
        }

        if let Some(k) = Kernel::get() {
            k.external_tools().replace_tools(&session_scope, &ext_tools).await;
        }

        scope_override = Some(session_scope);
    }

    if has_tool_results {
        let Some(ref scope) = scope_override else {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "Unsupported: tool result submission requires an Opencode session scope",
            );
        };

        let Some(thread_id) = state.store.get_active_thread(scope).ok().flatten() else {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "Unsupported: no active thread for this session scope",
            );
        };

        if request.stream {
            // Open reply stream BEFORE delivering results to avoid races.
            let response_stream =
                state.handler.stream_existing(Scope::from(scope.as_str()), thread_id).await;

            // Deliver tool results into the kernel (resumes head processing).
            // This must happen after stream_existing() so the reply stream is ready.
            for m in request.messages.iter().filter(|m| m.role == "tool") {
                let tool_call_id = m.tool_call_id.clone().unwrap_or_default();
                if tool_call_id.trim().is_empty() {
                    return openai_error(
                        StatusCode::BAD_REQUEST,
                        "Unsupported: tool messages must include tool_call_id",
                    );
                }
                let output = m.content.clone().unwrap_or_default();

                let Some(k) = Kernel::get() else {
                    return openai_error(StatusCode::INTERNAL_SERVER_ERROR, "Kernel not initialized");
                };
                if let Err(e) = k
                    .external_tools()
                    .deliver_result(scope, tool_call_id.trim(), output)
                    .await
                {
                    return openai_error(StatusCode::BAD_REQUEST, &format!("Unsupported: {e}"));
                }
            }

            let sse_stream = to_sse_stream(response_stream, request.model.clone());
            return Sse::new(sse_stream)
                .keep_alive(KeepAlive::default())
                .into_response();
        }

        // Open reply stream BEFORE delivering results to avoid races.
        let mut response_stream =
            state.handler.stream_existing(Scope::from(scope.as_str()), thread_id).await;

        for m in request.messages.iter().filter(|m| m.role == "tool") {
            let tool_call_id = m.tool_call_id.clone().unwrap_or_default();
            if tool_call_id.trim().is_empty() {
                return openai_error(
                    StatusCode::BAD_REQUEST,
                    "Unsupported: tool messages must include tool_call_id",
                );
            }
            let output = m.content.clone().unwrap_or_default();

            let Some(k) = Kernel::get() else {
                return openai_error(StatusCode::INTERNAL_SERVER_ERROR, "Kernel not initialized");
            };
            if let Err(e) = k
                .external_tools()
                .deliver_result(scope, tool_call_id.trim(), output)
                .await
            {
                return openai_error(StatusCode::BAD_REQUEST, &format!("Unsupported: {e}"));
            }
        }
        let mut content = String::new();
        let mut tool_call: Option<(String, String, String)> = None;
        while let Some(chunk) = response_stream.next().await {
            match chunk {
                ChatChunk::Delta(text) => content.push_str(&text),
                ChatChunk::ToolCall {
                    tool_call_id,
                    name,
                    arguments_json,
                } => {
                    tool_call = Some((tool_call_id, name, arguments_json));
                    break;
                }
                ChatChunk::Done => break,
                ChatChunk::Error(e) => {
                    return Json(serde_json::json!({
                        "error": {"message": e, "type": "server_error"}
                    }))
                    .into_response();
                }
            }
        }

        let (message, finish_reason) = if let Some((id, name, args)) = tool_call {
            (
                OpenAIResponseMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id,
                        call_type: "function".to_string(),
                        function: OpenAIToolCallFunction {
                            name,
                            arguments: args,
                        },
                    }]),
                },
                "tool_calls".to_string(),
            )
        } else {
            (
                OpenAIResponseMessage {
                    role: "assistant".to_string(),
                    content: Some(content),
                    tool_calls: None,
                },
                "stop".to_string(),
            )
        };

        let response = OpenAIChatResponse {
            id: response_id(),
            object: "chat.completion".to_string(),
            created: timestamp(),
            model: request.model.clone(),
            choices: vec![OpenAIChoice {
                index: 0,
                message,
                finish_reason,
            }],
            usage: OpenAIUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        };

        return Json(response).into_response();
    }

    let model = request.model.clone();
    let stream = request.stream;
    let chat_request = convert_request(request, scope_override);

    if stream {
        let response_stream = state.handler.handle_chat(chat_request).await;
        let sse_stream = to_sse_stream(response_stream, model);
        Sse::new(sse_stream)
            .keep_alive(KeepAlive::default())
            .into_response()
    } else {
        let mut response_stream = state.handler.handle_chat(chat_request).await;
        let mut content = String::new();
        let mut tool_call: Option<(String, String, String)> = None;

        while let Some(chunk) = response_stream.next().await {
            match chunk {
                ChatChunk::Delta(text) => content.push_str(&text),
                ChatChunk::ToolCall {
                    tool_call_id,
                    name,
                    arguments_json,
                } => {
                    tool_call = Some((tool_call_id, name, arguments_json));
                    break;
                }
                ChatChunk::Done => break,
                ChatChunk::Error(e) => {
                    return Json(serde_json::json!({
                        "error": {"message": e, "type": "server_error"}
                    }))
                    .into_response();
                }
            }
        }

        let (message, finish_reason) = if let Some((id, name, args)) = tool_call {
            (
                OpenAIResponseMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id,
                        call_type: "function".to_string(),
                        function: OpenAIToolCallFunction {
                            name,
                            arguments: args,
                        },
                    }]),
                },
                "tool_calls".to_string(),
            )
        } else {
            (
                OpenAIResponseMessage {
                    role: "assistant".to_string(),
                    content: Some(content),
                    tool_calls: None,
                },
                "stop".to_string(),
            )
        };

        let response = OpenAIChatResponse {
            id: response_id(),
            object: "chat.completion".to_string(),
            created: timestamp(),
            model,
            choices: vec![OpenAIChoice {
                index: 0,
                message,
                finish_reason,
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
    stream
        .scan((false, false), move |state, chunk| {
            let (sent_role, sent_tool_calls) = state;

            let event = match chunk {
                ChatChunk::Delta(content) => {
                    let delta = if !*sent_role {
                        *sent_role = true;
                        OpenAIDelta {
                            role: Some("assistant".to_string()),
                            content: Some(content),
                            tool_calls: None,
                        }
                    } else {
                        OpenAIDelta {
                            role: None,
                            content: Some(content),
                            tool_calls: None,
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
                ChatChunk::ToolCall {
                    tool_call_id,
                    name,
                    arguments_json,
                } => {
                    let role = if !*sent_role {
                        *sent_role = true;
                        Some("assistant".to_string())
                    } else {
                        None
                    };
                    *sent_tool_calls = true;
                    let chunk = OpenAIStreamChunk {
                        id: id.clone(),
                        object: "chat.completion.chunk".to_string(),
                        created,
                        model: model.clone(),
                        choices: vec![OpenAIStreamChoice {
                            index: 0,
                            delta: OpenAIDelta {
                                role,
                                content: None,
                                tool_calls: Some(vec![OpenAIStreamToolCallDelta {
                                    index: 0,
                                    id: Some(tool_call_id),
                                    call_type: Some("function".to_string()),
                                    function: Some(OpenAIToolCallFunction {
                                        name,
                                        arguments: arguments_json,
                                    }),
                                }]),
                            },
                            finish_reason: Some("tool_calls".to_string()),
                        }],
                    };

                    Event::default().data(serde_json::to_string(&chunk).unwrap())
                }
                ChatChunk::Done => {
                    if *sent_tool_calls {
                        return std::future::ready(None);
                    }
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
                                tool_calls: None,
                            },
                            finish_reason: Some("stop".to_string()),
                        }],
                    };

                    Event::default().data(serde_json::to_string(&chunk).unwrap())
                }
                ChatChunk::Error(e) => Event::default().data(format!("{{\"error\": \"{}\"}}", e)),
            };
            std::future::ready(Some(Ok(event)))
        })
        .chain(tokio_stream::once(Ok(Event::default().data("[DONE]"))))
}
