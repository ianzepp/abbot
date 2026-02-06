use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

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
        let v: serde_json::Value =
            serde_json::from_str(s).expect("invalid tool spec JSON");
        Self::function(
            v["name"].as_str().expect("missing tool spec name"),
            v["description"]
                .as_str()
                .expect("missing tool spec description"),
            v["parameters"].clone(),
        )
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
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
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
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let request = ChatRequest {
            model: model.to_string(),
            messages,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
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
}
