use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
pub struct OpenAICompatHttpError {
    pub status: u16,
    pub request_json: String,
    pub response_text: String,
}

impl fmt::Display for OpenAICompatHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "API error {}: {}", self.status, self.response_text)
    }
}

impl std::error::Error for OpenAICompatHttpError {}

#[derive(Debug)]
pub struct OpenAICompatTransportError {
    pub request_json: String,
    pub message: String,
}

impl fmt::Display for OpenAICompatTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transport error: {}", self.message)
    }
}

impl std::error::Error for OpenAICompatTransportError {}

#[derive(Debug)]
pub struct OpenAICompatDecodeError {
    pub request_json: String,
    pub response_text: String,
    pub message: String,
}

impl fmt::Display for OpenAICompatDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "decode error: {}", self.message)
    }
}

impl std::error::Error for OpenAICompatDecodeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunctionSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunctionSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value,
}

impl ToolSpec {
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: ToolFunctionSpec {
                name: name.into(),
                description: Some(description.into()),
                parameters,
            },
        }
    }

    pub fn from_json_str(s: &str) -> Self {
        let v: serde_json::Value = serde_json::from_str(s).expect("invalid tool spec JSON");
        let name = v["name"].as_str().expect("missing tool spec name");
        let parameters = &v["parameters"];
        validate_tool_schema(name, parameters);
        Self::function(
            name,
            v["description"]
                .as_str()
                .expect("missing tool spec description"),
            parameters.clone(),
        )
    }
}

/// Validate that a tool parameter schema is compatible with strict providers
/// (e.g., Ollama cloud models) that reject bare `{}`, missing `type`, or
/// objects without `properties`.
fn validate_tool_schema(tool_name: &str, schema: &Value) {
    validate_schema_node(tool_name, "$", schema);
}

fn validate_schema_node(tool: &str, path: &str, node: &Value) {
    let Some(obj) = node.as_object() else {
        return;
    };

    // An empty object `{}` is never valid as a schema node
    if obj.is_empty() {
        panic!("tool spec '{tool}' has empty schema {{}} at {path}");
    }

    // Every property must declare a "type"
    if obj.contains_key("description") && !obj.contains_key("type") && !obj.contains_key("$ref") {
        panic!("tool spec '{tool}' property at {path} has no 'type'");
    }

    // "type": "object" must have "properties"
    if obj.get("type").and_then(|t| t.as_str()) == Some("object")
        && path != "$"
        && !obj.contains_key("properties")
    {
        panic!("tool spec '{tool}' object at {path} has no 'properties'");
    }

    // "type": "array" items must not be empty
    if obj.get("type").and_then(|t| t.as_str()) == Some("array")
        && let Some(items) = obj.get("items")
    {
        if items.as_object().is_some_and(|o| o.is_empty()) {
            panic!("tool spec '{tool}' array at {path} has empty items {{}}");
        }
        validate_schema_node(tool, &format!("{path}.items"), items);
    }

    // Recurse into properties
    if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
        for (key, val) in props {
            validate_schema_node(tool, &format!("{path}.{key}"), val);
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    // Newer OpenAI models (notably GPT-5 chat variants) may reject max_tokens in favor
    // of max_completion_tokens.
    #[serde(
        rename = "max_completion_tokens",
        skip_serializing_if = "Option::is_none"
    )]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
}

#[derive(Debug, Serialize)]
struct CompletionRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct CompletionResponse {
    choices: Vec<CompletionChoice>,
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct CompletionChoice {
    text: String,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ChatResult {
    pub content: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone)]
pub struct ChatToolResult {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
    pub request_json: String,
    pub response_json: String,
}

#[derive(Clone)]
pub struct OpenAICompatClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    extra_headers: Vec<(String, String)>,
    // Some OpenAI models require `max_completion_tokens` instead of `max_tokens`.
    // Cache models that have returned that error so we don't pay a failing request
    // on every call.
    models_require_max_completion_tokens: Arc<Mutex<HashSet<String>>>,
}

impl OpenAICompatClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            temperature,
            max_tokens,
            extra_headers,
            models_require_max_completion_tokens: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn should_use_max_completion_tokens(&self, model: &str) -> bool {
        self.models_require_max_completion_tokens
            .lock()
            .ok()
            .is_some_and(|set| set.contains(model))
    }

    fn note_requires_max_completion_tokens(&self, model: &str) {
        if let Ok(mut set) = self.models_require_max_completion_tokens.lock() {
            set.insert(model.to_string());
        }
    }

    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<ChatResult, Error> {
        let res = self.chat_with_tools(messages, None, None).await?;
        Ok(ChatResult {
            content: res.content.unwrap_or_else(|| "(no response)".to_string()),
            usage: res.usage,
        })
    }

    pub async fn chat_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<ToolSpec>>,
        tool_choice: Option<Value>,
    ) -> Result<ChatToolResult, Error> {
        self.chat_with_tools_on_model(self.model.as_str(), messages, tools, tool_choice)
            .await
    }

    pub async fn chat_with_tools_on_model(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<ToolSpec>>,
        tool_choice: Option<Value>,
    ) -> Result<ChatToolResult, Error> {
        match self
            .try_chat_completions(model, messages.clone(), tools.clone(), tool_choice.clone())
            .await
        {
            Ok(r) => Ok(r),
            Err(e) => {
                // Some models (e.g. legacy Codex / instruct-style) are completion-only and will
                // reject /v1/chat/completions. If we see that specific failure and the request
                // does not involve tool calls, retry via /v1/completions.
                if let Some(http) = e.downcast_ref::<OpenAICompatHttpError>()
                    && is_non_chat_model_error(http.status, &http.response_text)
                    && can_fallback_to_completions(&messages, &tools, &tool_choice)
                {
                    return self.completions(model, messages).await;
                }
                Err(e)
            }
        }
    }

    async fn try_chat_completions(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<ToolSpec>>,
        tool_choice: Option<Value>,
    ) -> Result<ChatToolResult, Error> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let use_mct = self.should_use_max_completion_tokens(model);

        let request = ChatRequest {
            model: model.to_string(),
            messages: messages.clone(),
            temperature: self.temperature,
            max_tokens: if use_mct { None } else { self.max_tokens },
            max_completion_tokens: if use_mct { self.max_tokens } else { None },
            tools: tools.clone(),
            tool_choice: tool_choice.clone(),
        };

        let request_json = serde_json::to_string(&request)?;

        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json");

        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        for (k, v) in &self.extra_headers {
            req = req.header(k, v);
        }

        let response = match req.body(request_json.clone()).send().await {
            Ok(r) => r,
            Err(e) => {
                return Err(Box::new(OpenAICompatTransportError {
                    request_json,
                    message: e.to_string(),
                }));
            }
        };
        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            let http_err = OpenAICompatHttpError {
                status: status.as_u16(),
                request_json,
                response_text,
            };

            // Some OpenAI models reject max_tokens and require max_completion_tokens.
            if is_max_tokens_unsupported_error(http_err.status, &http_err.response_text)
                && self.max_tokens.is_some()
                && !use_mct
            {
                self.note_requires_max_completion_tokens(model);
                return self
                    .try_chat_completions_with_max_completion_tokens(
                        model,
                        messages,
                        tools,
                        tool_choice,
                    )
                    .await;
            }

            return Err(Box::new(http_err));
        }

        let chat_response: ChatResponse = serde_json::from_str(&response_text).map_err(|e| {
            Box::new(OpenAICompatDecodeError {
                request_json: request_json.clone(),
                response_text: response_text.clone(),
                message: e.to_string(),
            }) as Error
        })?;

        let msg = chat_response.choices.first().map(|c| &c.message);

        let content = msg.and_then(|m| m.content.clone());
        let tool_calls = msg.map(|m| m.tool_calls.clone()).unwrap_or_default();

        Ok(ChatToolResult {
            content,
            tool_calls,
            usage: chat_response.usage,
            request_json,
            response_json: response_text,
        })
    }

    async fn try_chat_completions_with_max_completion_tokens(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<ToolSpec>>,
        tool_choice: Option<Value>,
    ) -> Result<ChatToolResult, Error> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let request = ChatRequest {
            model: model.to_string(),
            messages,
            temperature: self.temperature,
            max_tokens: None,
            max_completion_tokens: self.max_tokens,
            tools,
            tool_choice,
        };

        let request_json = serde_json::to_string(&request)?;

        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json");

        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        for (k, v) in &self.extra_headers {
            req = req.header(k, v);
        }

        let response = match req.body(request_json.clone()).send().await {
            Ok(r) => r,
            Err(e) => {
                return Err(Box::new(OpenAICompatTransportError {
                    request_json,
                    message: e.to_string(),
                }));
            }
        };
        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(Box::new(OpenAICompatHttpError {
                status: status.as_u16(),
                request_json,
                response_text,
            }));
        }

        let chat_response: ChatResponse = serde_json::from_str(&response_text).map_err(|e| {
            Box::new(OpenAICompatDecodeError {
                request_json: request_json.clone(),
                response_text: response_text.clone(),
                message: e.to_string(),
            }) as Error
        })?;

        let msg = chat_response.choices.first().map(|c| &c.message);
        let content = msg.and_then(|m| m.content.clone());
        let tool_calls = msg.map(|m| m.tool_calls.clone()).unwrap_or_default();

        Ok(ChatToolResult {
            content,
            tool_calls,
            usage: chat_response.usage,
            request_json,
            response_json: response_text,
        })
    }

    async fn completions(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
    ) -> Result<ChatToolResult, Error> {
        let url = format!("{}/completions", self.base_url.trim_end_matches('/'));
        let prompt = messages_to_prompt(&messages)?;

        let request = CompletionRequest {
            model: model.to_string(),
            prompt,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
        };

        let request_json = serde_json::to_string(&request)?;

        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json");

        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        for (k, v) in &self.extra_headers {
            req = req.header(k, v);
        }

        let response = match req.body(request_json.clone()).send().await {
            Ok(r) => r,
            Err(e) => {
                return Err(Box::new(OpenAICompatTransportError {
                    request_json,
                    message: e.to_string(),
                }));
            }
        };

        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(Box::new(OpenAICompatHttpError {
                status: status.as_u16(),
                request_json,
                response_text,
            }));
        }

        let completion_response: CompletionResponse = serde_json::from_str(&response_text)
            .map_err(|e| {
                Box::new(OpenAICompatDecodeError {
                    request_json: request_json.clone(),
                    response_text: response_text.clone(),
                    message: e.to_string(),
                }) as Error
            })?;

        let text = completion_response
            .choices
            .first()
            .map(|c| c.text.clone())
            .unwrap_or_default();

        Ok(ChatToolResult {
            content: Some(text),
            tool_calls: Vec::new(),
            usage: completion_response.usage,
            request_json,
            response_json: response_text,
        })
    }
}

fn is_non_chat_model_error(status: u16, response_text: &str) -> bool {
    // Providers differ, but OpenAI returns a 400 JSON error like:
    // "This is not a chat model and thus not supported in the v1/chat/completions endpoint..."
    if status != 400 && status != 404 {
        return false;
    }
    let t = response_text;
    t.contains("not a chat model") && (t.contains("v1/completions") || t.contains("/completions"))
}

fn is_max_tokens_unsupported_error(status: u16, response_text: &str) -> bool {
    if status != 400 {
        return false;
    }
    // OpenAI error string (example):
    // "Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead."
    response_text.contains("Unsupported parameter")
        && response_text.contains("max_tokens")
        && response_text.contains("max_completion_tokens")
}

fn can_fallback_to_completions(
    messages: &[ChatMessage],
    tools: &Option<Vec<ToolSpec>>,
    tool_choice: &Option<Value>,
) -> bool {
    if tools.is_some() || tool_choice.is_some() {
        return false;
    }

    for m in messages {
        if m.role == Role::Tool {
            return false;
        }
        if m.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()) {
            return false;
        }
    }

    true
}

fn messages_to_prompt(messages: &[ChatMessage]) -> Result<String, Error> {
    // Minimal, stable translation of chat history into a single prompt.
    // This is only used for completion-only models; tool calls are not supported here.
    let mut out = String::new();
    let mut saw_system = false;

    for m in messages {
        if m.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()) {
            return Err("cannot convert tool_calls to /v1/completions prompt".into());
        }
        if m.role == Role::Tool {
            return Err("cannot convert tool role messages to /v1/completions prompt".into());
        }

        let content = m.content.as_deref().unwrap_or("");
        match m.role {
            Role::System => {
                saw_system = true;
                out.push_str("### System\n");
                out.push_str(content);
                out.push_str("\n\n");
            }
            Role::User => {
                out.push_str("### User\n");
                out.push_str(content);
                out.push_str("\n\n");
            }
            Role::Assistant => {
                out.push_str("### Assistant\n");
                out.push_str(content);
                out.push_str("\n\n");
            }
            Role::Tool => {}
        }
    }

    // Ensure there's always a completion target.
    if !out.ends_with("### Assistant\n") && (!out.is_empty() || !saw_system) {
        out.push_str("### Assistant\n");
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_chat_model_error_detection() {
        let msg = "{\"error\":{\"message\":\"This is not a chat model and thus not supported in the v1/chat/completions endpoint. Did you mean to use v1/completions\"}}";
        assert!(is_non_chat_model_error(400, msg));
        assert!(!is_non_chat_model_error(500, msg));
    }

    #[test]
    fn messages_to_prompt_basic() {
        let messages = vec![
            ChatMessage::new(Role::System, "You are helpful."),
            ChatMessage::new(Role::User, "What is 2+2?"),
        ];
        let prompt = messages_to_prompt(&messages).unwrap();
        assert!(prompt.contains("### System\nYou are helpful."));
        assert!(prompt.contains("### User\nWhat is 2+2?"));
        assert!(prompt.ends_with("### Assistant\n"));
    }

    #[test]
    fn messages_to_prompt_rejects_tool_role() {
        let messages = vec![ChatMessage::tool_result("t1", "ok")];
        assert!(messages_to_prompt(&messages).is_err());
    }
}
