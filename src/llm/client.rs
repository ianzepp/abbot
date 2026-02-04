use serde_json::Value;

use super::anthropic::{self, AnthropicClient};
use super::openai_compat::{self, OpenAICompatClient};
use crate::runtime::AppConfig;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// Unified tool specification that works with both providers.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    fn to_openai(&self) -> openai_compat::ToolSpec {
        openai_compat::ToolSpec::function(&self.name, &self.description, self.parameters.clone())
    }

    fn to_anthropic(&self) -> anthropic::ToolSpec {
        anthropic::ToolSpec::new(&self.name, &self.description, self.parameters.clone())
    }
}

/// Unified tool call result from either provider.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Unified message for conversation history.
#[derive(Debug, Clone)]
pub enum Message {
    System(String),
    User(String),
    Assistant(String),
    AssistantToolCalls(Vec<ToolCall>),
    ToolResult {
        id: String,
        content: String,
        is_error: bool,
    },
}

/// Unified usage statistics.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Result of a chat completion with tool support.
#[derive(Debug, Clone)]
pub struct ChatToolResult {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub request_json: String,
    pub response_json: String,
}

impl ChatToolResult {
    pub fn has_tool_use(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// Unified LLM client that dispatches to OpenAI-compatible or Anthropic backends.
#[derive(Clone)]
pub enum LlmClient {
    OpenAI(OpenAICompatClient),
    Anthropic(AnthropicClient),
}

impl LlmClient {
    /// Create client from model ID (e.g., "anthropic/claude-sonnet-4-20250514"),
    /// resolving provider, base_url, and api_key from abbot.toml `[providers.*]`.
    pub fn from_model_id(model_id: &str) -> Result<Self, Error> {
        Self::from_model_id_with_options(model_id, None, None)
    }

    /// Create client from model ID with optional temperature and max_tokens overrides.
    pub fn from_model_id_with_options(
        model_id: &str,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<Self, Error> {
        let (provider, api_model) = parse_model_id(model_id);
        if provider.is_empty() || api_model.is_empty() {
            return Err(format!("invalid model id: {}", model_id).into());
        }

        let app = AppConfig::global();
        let cfg = app
            .providers
            .get(&provider)
            .ok_or_else(|| format!("provider not configured: {}", provider))?;
        let base_url = cfg.base_url.as_deref().unwrap_or("");

        let api_key = cfg
            .api_key_env
            .as_deref()
            .and_then(|k| {
                let k = k.trim();
                if k.is_empty() {
                    None
                } else {
                    std::env::var(k).ok()
                }
            })
            .unwrap_or_default();

        Ok(Self::new(
            &provider,
            base_url,
            &api_key,
            &api_model,
            temperature,
            max_tokens,
            vec![],
        ))
    }

    /// Create a new client based on provider name.
    /// Supported providers: "openai", "anthropic", "ollama" (uses OpenAI-compat)
    pub fn new(
        provider: &str,
        base_url: &str,
        api_key: &str,
        model: &str,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        match provider {
            "anthropic" => {
                let client =
                    AnthropicClient::new(api_key, model, max_tokens.unwrap_or(4096), temperature);
                let client = if !base_url.is_empty() {
                    client.with_base_url(base_url.trim_end_matches("/v1"))
                } else {
                    client
                };
                LlmClient::Anthropic(client)
            }
            _ => {
                let client = OpenAICompatClient::new(
                    base_url,
                    api_key,
                    model,
                    temperature,
                    max_tokens,
                    extra_headers,
                );
                LlmClient::OpenAI(client)
            }
        }
    }

    /// Simple chat without tools.
    pub async fn chat(&self, messages: Vec<Message>) -> Result<String, Error> {
        let result = self.chat_with_tools(messages, None).await?;
        Ok(result
            .content
            .unwrap_or_else(|| "(no response)".to_string()))
    }

    /// Chat with optional tool support.
    pub async fn chat_with_tools(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolSpec>>,
    ) -> Result<ChatToolResult, Error> {
        match self {
            LlmClient::OpenAI(client) => {
                let (oai_messages, _system) = to_openai_messages(messages);
                let oai_tools = tools
                    .as_ref()
                    .map(|t| t.iter().map(|s| s.to_openai()).collect());

                let result = client
                    .chat_with_tools(oai_messages, oai_tools, None)
                    .await?;

                Ok(ChatToolResult {
                    content: result.content,
                    tool_calls: result
                        .tool_calls
                        .into_iter()
                        .map(|tc| ToolCall {
                            id: tc.id,
                            name: tc.function.name,
                            arguments: serde_json::from_str(&tc.function.arguments)
                                .unwrap_or(Value::Null),
                        })
                        .collect(),
                    usage: Usage {
                        input_tokens: result
                            .usage
                            .as_ref()
                            .and_then(|u| u.prompt_tokens)
                            .unwrap_or(0),
                        output_tokens: result
                            .usage
                            .as_ref()
                            .and_then(|u| u.completion_tokens)
                            .unwrap_or(0),
                    },
                    request_json: result.request_json,
                    response_json: result.response_json,
                })
            }
            LlmClient::Anthropic(client) => {
                let (ant_messages, system) = to_anthropic_messages(messages);
                let ant_tools = tools
                    .as_ref()
                    .map(|t| t.iter().map(|s| s.to_anthropic()).collect());

                let result = client
                    .chat_with_tools(system, ant_messages, ant_tools, None)
                    .await?;

                Ok(ChatToolResult {
                    content: result.content,
                    tool_calls: result
                        .tool_calls
                        .into_iter()
                        .map(|tc| ToolCall {
                            id: tc.id,
                            name: tc.name,
                            arguments: tc.input,
                        })
                        .collect(),
                    usage: Usage {
                        input_tokens: result.usage.input_tokens,
                        output_tokens: result.usage.output_tokens,
                    },
                    request_json: result.request_json,
                    response_json: result.response_json,
                })
            }
        }
    }
}

fn to_openai_messages(messages: Vec<Message>) -> (Vec<openai_compat::ChatMessage>, Option<String>) {
    let mut system = None;
    let mut out = Vec::new();

    for msg in messages {
        match msg {
            Message::System(s) => {
                system = Some(s.clone());
                out.push(openai_compat::ChatMessage::new(
                    openai_compat::Role::System,
                    s,
                ));
            }
            Message::User(s) => {
                out.push(openai_compat::ChatMessage::new(
                    openai_compat::Role::User,
                    s,
                ));
            }
            Message::Assistant(s) => {
                out.push(openai_compat::ChatMessage::new(
                    openai_compat::Role::Assistant,
                    s,
                ));
            }
            Message::AssistantToolCalls(calls) => {
                let oai_calls: Vec<openai_compat::ToolCall> = calls
                    .into_iter()
                    .map(|tc| openai_compat::ToolCall {
                        id: tc.id,
                        call_type: "function".to_string(),
                        function: openai_compat::ToolCallFunction {
                            name: tc.name,
                            arguments: tc.arguments.to_string(),
                        },
                    })
                    .collect();
                out.push(openai_compat::ChatMessage::assistant_tool_calls(oai_calls));
            }
            Message::ToolResult { id, content, .. } => {
                out.push(openai_compat::ChatMessage::tool_result(id, content));
            }
        }
    }

    (out, system)
}

fn to_anthropic_messages(messages: Vec<Message>) -> (Vec<anthropic::Message>, Option<String>) {
    let mut system = None;
    let mut out = Vec::new();

    for msg in messages {
        match msg {
            Message::System(s) => {
                system = Some(s);
            }
            Message::User(s) => {
                out.push(anthropic::Message::user(s));
            }
            Message::Assistant(s) => {
                out.push(anthropic::Message::assistant(s));
            }
            Message::AssistantToolCalls(calls) => {
                let blocks: Vec<anthropic::ContentBlock> = calls
                    .into_iter()
                    .map(|tc| anthropic::ContentBlock::ToolUse {
                        id: tc.id,
                        name: tc.name,
                        input: tc.arguments,
                    })
                    .collect();
                out.push(anthropic::Message::with_content(
                    anthropic::Role::Assistant,
                    blocks,
                ));
            }
            Message::ToolResult {
                id,
                content,
                is_error,
            } => {
                out.push(anthropic::Message::tool_result(id, content, is_error));
            }
        }
    }

    (out, system)
}

fn parse_model_id(model_id: &str) -> (String, String) {
    let model_id = model_id.trim().trim_matches('/');
    if model_id.is_empty() {
        return (String::new(), String::new());
    }

    let mut parts = model_id.split('/');
    let provider = parts.next().unwrap_or("").to_string();
    if provider.is_empty() {
        return (String::new(), model_id.to_string());
    }

    if provider == "openrouter" {
        let rest: Vec<&str> = parts.collect();
        return (provider, rest.join("/"));
    }

    (provider, model_id.split('/').last().unwrap_or(model_id).to_string())
}
