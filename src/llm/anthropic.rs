use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
pub struct AnthropicHttpError {
    pub status: u16,
    pub request_json: String,
    pub response_text: String,
}

impl fmt::Display for AnthropicHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Anthropic API error {}: {}",
            self.status, self.response_text
        )
    }
}

impl std::error::Error for AnthropicHttpError {}

#[derive(Debug)]
pub struct AnthropicTransportError {
    pub request_json: String,
    pub message: String,
}

impl fmt::Display for AnthropicTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transport error: {}", self.message)
    }
}

impl std::error::Error for AnthropicTransportError {}

#[derive(Debug)]
pub struct AnthropicDecodeError {
    pub request_json: String,
    pub response_text: String,
    pub message: String,
}

impl fmt::Display for AnthropicDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "decode error: {}", self.message)
    }
}

impl std::error::Error for AnthropicDecodeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant_tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: Value,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
            }],
        }
    }

    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: content.into(),
                is_error: if is_error { Some(true) } else { None },
            }],
        }
    }

    pub fn with_content(role: Role, content: Vec<ContentBlock>) -> Self {
        Self { role, content }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

impl ToolSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            input_schema,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoice>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    Any,
    Tool { name: String },
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    content: Vec<ResponseContent>,
    stop_reason: Option<String>,
    usage: ResponseUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Deserialize)]
struct ResponseUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ChatResult {
    pub content: String,
    pub usage: Usage,
}

#[derive(Debug, Clone)]
pub struct ChatToolResult {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: Option<String>,
    pub usage: Usage,
    pub request_json: String,
    pub response_json: String,
}

impl ChatToolResult {
    pub fn has_tool_use(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

#[derive(Clone)]
pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: u32,
    temperature: Option<f32>,
    api_version: String,
}

impl AnthropicClient {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        max_tokens: u32,
        temperature: Option<f32>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: api_key.into(),
            model: model.into(),
            max_tokens,
            temperature,
            api_version: "2023-06-01".to_string(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_api_version(mut self, version: impl Into<String>) -> Self {
        self.api_version = version.into();
        self
    }

    pub async fn chat(
        &self,
        system: Option<String>,
        messages: Vec<Message>,
    ) -> Result<ChatResult, Error> {
        let res = self.chat_with_tools(system, messages, None, None).await?;
        Ok(ChatResult {
            content: res.content.unwrap_or_else(|| "(no response)".to_string()),
            usage: res.usage,
        })
    }

    pub async fn chat_with_tools(
        &self,
        system: Option<String>,
        messages: Vec<Message>,
        tools: Option<Vec<ToolSpec>>,
        tool_choice: Option<ToolChoice>,
    ) -> Result<ChatToolResult, Error> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));

        let request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system,
            messages,
            temperature: self.temperature,
            tools,
            tool_choice,
        };

        let request_json = serde_json::to_string(&request)?;

        let response = match self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.api_version)
            .body(request_json.clone())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Err(Box::new(AnthropicTransportError {
                    request_json,
                    message: e.to_string(),
                }));
            }
        };

        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(Box::new(AnthropicHttpError {
                status: status.as_u16(),
                request_json,
                response_text,
            }));
        }

        let messages_response: MessagesResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                Box::new(AnthropicDecodeError {
                    request_json: request_json.clone(),
                    response_text: response_text.clone(),
                    message: e.to_string(),
                }) as Error
            })?;

        let mut text_content: Option<String> = None;
        let mut tool_calls = Vec::new();

        for block in messages_response.content {
            match block {
                ResponseContent::Text { text } => {
                    text_content = Some(text);
                }
                ResponseContent::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall { id, name, input });
                }
            }
        }

        Ok(ChatToolResult {
            content: text_content,
            tool_calls,
            stop_reason: messages_response.stop_reason,
            usage: Usage {
                input_tokens: messages_response.usage.input_tokens,
                output_tokens: messages_response.usage.output_tokens,
            },
            request_json,
            response_json: response_text,
        })
    }
}
