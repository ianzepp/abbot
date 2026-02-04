// OpenAI-compatible API endpoint.
//
// Implements GET /v1/models and POST /v1/chat/completions with SSE streaming.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use tokio_stream::Stream;

use super::IngressHub;
use super::handler::{ChatChunk, ChatMessage, ChatRequest, Role};
use super::user_prompt::process_user_system_prompt;
use crate::history::{Store, ToolRegistryTool};
use crate::runtime::Kernel;

use super::session_scope::{bearer_token, extract_env_block, extract_env_cwd, session_scope_from};

const MODEL_ID: &str = "abbot/default";

fn openai_error(status: StatusCode, message: impl Into<String>) -> Response {
    let message = message.into();
    let error_type = match status.as_u16() {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        _ if status.is_server_error() => "server_error",
        _ => "api_error",
    };
    (
        status,
        Json(serde_json::json!({
            "error": {
                "message": message,
                "type": error_type
            }
        })),
    )
        .into_response()
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

fn is_localhost_request(headers: &HeaderMap) -> bool {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    host.starts_with("127.0.0.1") || host.starts_with("localhost") || host.starts_with("[::1]")
}

fn extract_env_block_from_system(req: &OpenAIChatRequest) -> Option<String> {
    let system = req.messages.iter().find(|m| m.role == "system")?;
    let content = system.content.as_deref()?;
    extract_env_block(content)
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
    pub ingress: Arc<IngressHub>,
    pub store: Arc<Store>,
    pub proxy: bool,
    proxy_chat: Option<Arc<ProxyChat>>,
}

impl OpenAIState {
    pub fn new(store: Arc<Store>, head_id: &str) -> Self {
        Self {
            ingress: Arc::new(IngressHub::new(store.clone(), head_id)),
            store,
            proxy: false,
            proxy_chat: None,
        }
    }

    pub fn with_proxy(mut self, proxy: bool) -> Self {
        self.proxy = proxy;
        if self.proxy {
            self.proxy_chat = ProxyChat::from_env_or_config().ok().map(Arc::new);
        }
        self
    }
}

#[derive(Clone)]
struct ProxyChat {
    client: reqwest::Client,
    base_url: String,
}

impl ProxyChat {
    fn from_env_or_config() -> Result<Self, String> {
        let base_url = crate::runtime::AppConfig::global()
            .server
            .proxy_base_url
            .clone()
            .unwrap_or_default();

        if base_url.trim().is_empty() {
            return Err("proxy mode requires server.proxy_base_url in abbot.toml".to_string());
        }

        Ok(Self {
            client: reqwest::Client::new(),
            base_url,
        })
    }

    fn url(&self, suffix: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), suffix)
    }

    async fn proxy_models(&self, headers: &HeaderMap) -> Result<Response, String> {
        let url = self.url("/models");
        let mut req = self.client.get(url);
        if let Some(auth) = headers.get("authorization") {
            req = req.header("authorization", auth.clone());
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let mut out = Response::builder().status(status);

        for (k, v) in resp.headers().iter() {
            if k.as_str().eq_ignore_ascii_case("content-length")
                || k.as_str().eq_ignore_ascii_case("transfer-encoding")
                || k.as_str().eq_ignore_ascii_case("connection")
            {
                continue;
            }
            out = out.header(k, v);
        }

        let body = resp.bytes().await.map_err(|e| e.to_string())?;
        out.body(Body::from(body)).map_err(|e| e.to_string())
    }

    async fn proxy_chat_completions(
        &self,
        headers: &HeaderMap,
        request_json: serde_json::Value,
        stream: bool,
    ) -> Result<Response, String> {
        let url = self.url("/chat/completions");
        let mut req = self.client.post(url);

        // Forward Authorization as provided (transparent proxy auth).
        if let Some(auth) = headers.get("authorization") {
            req = req.header("authorization", auth.clone());
        }

        // Forward a small set of common OpenAI headers if present.
        for name in ["openai-organization", "openai-project"] {
            if let Some(v) = headers.get(name) {
                req = req.header(name, v.clone());
            }
        }

        // Forward request.
        let resp = req
            .json(&request_json)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        let mut out = Response::builder().status(status);

        for (k, v) in resp.headers().iter() {
            if k.as_str().eq_ignore_ascii_case("content-length")
                || k.as_str().eq_ignore_ascii_case("transfer-encoding")
                || k.as_str().eq_ignore_ascii_case("connection")
            {
                continue;
            }
            out = out.header(k, v);
        }

        // Ensure content-type is present for both JSON and SSE.
        if out
            .headers_ref()
            .and_then(|h| h.get("content-type"))
            .is_none()
        {
            if stream {
                out = out.header(
                    "content-type",
                    HeaderValue::from_static("text/event-stream"),
                );
            } else {
                out = out.header("content-type", HeaderValue::from_static("application/json"));
            }
        }

        if stream {
            let body_stream = resp
                .bytes_stream()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
            let body = Body::from_stream(body_stream);
            out.body(body).map_err(|e| e.to_string())
        } else {
            let body = resp.bytes().await.map_err(|e| e.to_string())?;
            out.body(Body::from(body)).map_err(|e| e.to_string())
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

pub async fn list_models(State(state): State<OpenAIState>, headers: HeaderMap) -> Response {
    log_headers("GET /v1/models", &headers);

    if state.proxy {
        let Some(proxy) = state.proxy_chat.as_ref() else {
            return openai_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "proxy mode is enabled but server.proxy_base_url is not configured",
            );
        };
        return match proxy.proxy_models(&headers).await {
            Ok(r) => r,
            Err(e) => openai_error(StatusCode::BAD_GATEWAY, format!("upstream error: {e}")),
        };
    }

    Json(OpenAIModelsResponse {
        object: "list".to_string(),
        data: vec![OpenAIModel {
            id: MODEL_ID.to_string(),
            object: "model".to_string(),
            created: timestamp(),
            owned_by: "abbot".to_string(),
        }],
    })
    .into_response()
}

pub async fn chat_completions(
    State(state): State<OpenAIState>,
    headers: HeaderMap,
    Json(request_json): Json<serde_json::Value>,
) -> Response {
    log_headers("POST /v1/chat/completions", &headers);

    if state.proxy {
        let Some(proxy) = state.proxy_chat.as_ref() else {
            return openai_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "proxy mode is enabled but server.proxy_base_url is not configured",
            );
        };

        let stream = request_json
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        return match proxy
            .proxy_chat_completions(&headers, request_json, stream)
            .await
        {
            Ok(r) => r,
            Err(e) => openai_error(StatusCode::BAD_GATEWAY, format!("upstream error: {e}")),
        };
    }

    let request: OpenAIChatRequest = match serde_json::from_value(request_json) {
        Ok(r) => r,
        Err(e) => return openai_error(StatusCode::BAD_REQUEST, format!("invalid request: {e}")),
    };
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

    // All non-proxy requests must be session-scoped.

    // If this is a tool-result continuation turn (OpenCode), we don't create a new need.
    let last_non_system_role = request
        .messages
        .iter()
        .rev()
        .find(|m| m.role != "system")
        .map(|m| m.role.as_str());

    // Treat this request as a tool-result submission only when the last non-system
    // message(s) are tool messages. Clients (like Opencode) include tool messages in
    // subsequent turns for context; those must NOT be re-delivered.
    let is_tool_submission = matches!(last_non_system_role, Some("tool"));

    // Check for localhost admin requests (e.g., from TUI)
    let is_localhost = is_localhost_request(&headers);
    let has_opencode_marker = contains_opencode_marker(&request);

    let (scope, _cwd) = if is_localhost && !has_opencode_marker {
        // Localhost requests without opencode marker are treated as admin requests
        tracing::info!("localhost admin request to main scope");
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        ("main".to_string(), cwd)
    } else {
        // Opencode client path - requires marker, auth, and env block
        if !has_opencode_marker {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "Unsupported: requests require an Opencode session scope",
            );
        }

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
        let Some(cwd_str) = extract_env_cwd(&env_block) else {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "Unsupported: Opencode support requires a Working directory in the <env> block",
            );
        };

        let scope = session_scope_from(token, &cwd_str);
        tracing::info!(scope = %scope, client_cwd = %cwd_str, "opencode session scope derived");
        (scope, std::path::PathBuf::from(cwd_str))
    };

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

    let tools_json = ext_tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "summary": t.summary,
                "description": t.description,
                "schema_json": t.schema_json,
            })
        })
        .collect::<Vec<_>>();

    let Some(k) = Kernel::get() else {
        return openai_error(StatusCode::INTERNAL_SERVER_ERROR, "Kernel not initialized");
    };

    let dispatcher = k.dispatcher().await;
    let req = crate::kernel::Frame::req(
        "tool:register",
        serde_json::json!({
            "scope": scope,
            "tools": tools_json,
        }),
    )
    .with_actor("server/openai");

    let mut rx = dispatcher.dispatch(
        req,
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        tokio_util::sync::CancellationToken::new(),
    );
    let _ = rx.recv().await;

    tracing::info!(scope = %scope, tool_count = ext_tools.len(), "external tools registered");

    // `scope` is the session/<hash> scope for this request.

    let system_prompt = request
        .messages
        .iter()
        .find(|m| m.role == "system")
        .and_then(|m| m.content.clone());

    if is_tool_submission {
        let scope = scope.as_str();

        // Only ingest the trailing tool messages for this submission (the ones that correspond
        // to the immediately preceding tool call(s)).
        let mut tool_results = Vec::new();
        for m in request
            .messages
            .iter()
            .rev()
            .take_while(|m| m.role == "tool")
        {
            tool_results.push((
                m.tool_call_id.clone().unwrap_or_default(),
                m.content.clone().unwrap_or_default(),
            ));
        }
        tool_results.reverse();

        let response_stream = match state
            .ingress
            .submit_tool_results(scope, tool_results, request.stream)
            .await
        {
            Ok(s) => s,
            Err((code, msg)) => return openai_error(code, msg),
        };

        if request.stream {
            let sse_stream = to_sse_stream(response_stream, request.model.clone());
            return Sse::new(sse_stream)
                .keep_alive(KeepAlive::default())
                .into_response();
        }

        let mut response_stream = response_stream;
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
                    return openai_error(StatusCode::INTERNAL_SERVER_ERROR, e);
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

    if !is_tool_submission {
        if let Some(ref prompt_text) = system_prompt {
            if let Err(err) = process_user_system_prompt(
                state.store.clone(),
                scope.as_str(),
                prompt_text,
                &ext_tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            )
            .await
            {
                tracing::warn!(scope = %scope, error = %err, "failed to cache user system prompt");
            }
        }
    }

    let chat_request = convert_request(request, Some(scope.clone()));

    if stream {
        let response_stream = state
            .ingress
            .submit_user_turn(scope.as_str(), chat_request)
            .await;
        let sse_stream = to_sse_stream(response_stream, model);
        Sse::new(sse_stream)
            .keep_alive(KeepAlive::default())
            .into_response()
    } else {
        let mut response_stream = state
            .ingress
            .submit_user_turn(scope.as_str(), chat_request)
            .await;
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
                    return openai_error(StatusCode::INTERNAL_SERVER_ERROR, e);
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
