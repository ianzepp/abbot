/// Generic OpenAI-compatible client using reqwest.
/// Works with any backend that implements the /chat/completions API.

use serde::{Deserialize, Serialize};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

pub struct OpenAICompatClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAICompatClient {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<String, Error> {
        let url = format!("{}/chat/completions", self.base_url);

        let request = ChatRequest {
            model: self.model.clone(),
            messages,
        };

        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json");

        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = req.json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("API error {}: {}", status, body).into());
        }

        let chat_response: ChatResponse = response.json().await?;

        let content = chat_response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_else(|| "(no response)".to_string());

        Ok(content)
    }
}
