use crate::bus::Message;
use super::backend::Backend;
use super::openai_compat::{OpenAICompatClient, ChatMessage, Role, Error};

pub struct LlmClient {
    client: OpenAICompatClient,
}

impl LlmClient {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let client = OpenAICompatClient::new(base_url, api_key, model);
        Self { client }
    }

    /// Create client from environment, routing based on model prefix.
    /// Model format: "backend/model-name"
    /// Examples:
    /// - "openrouter/anthropic/claude-sonnet-4.5"
    /// - "openai/gpt-4"
    /// - "zai/glm-4.7"
    /// - "ollama/llama3"
    pub fn from_env(model: &str) -> Result<Self, Error> {
        let (backend, model_name) = Backend::from_model_prefix(model);

        // Ollama local doesn't require an API key
        let api_key = std::env::var(backend.api_key_env()).unwrap_or_default();
        if api_key.is_empty() && backend != Backend::Ollama {
            return Err(format!("{} not set", backend.api_key_env()).into());
        }

        Ok(Self::new(backend.base_url(), &api_key, model_name))
    }

    pub async fn chat(
        &self,
        system_prompt: &str,
        user_message: &str,
        history: &[Message],
    ) -> Result<String, Error> {
        let mut messages = vec![ChatMessage::new(Role::System, system_prompt)];

        for msg in history {
            let content = match msg.text() {
                Some(text) => text,
                None => continue,
            };

            // Exclude ping/pong from history
            if content == "<ping/>" || content == "<pong/>" {
                continue;
            }

            let role = if msg.sender == "abbot" {
                Role::Assistant
            } else {
                Role::User
            };

            let formatted = if role == Role::User {
                format!("<{}> {}", msg.sender, content)
            } else {
                content.to_string()
            };

            messages.push(ChatMessage::new(role, formatted));
        }

        messages.push(ChatMessage::new(Role::User, user_message));

        self.client.chat(messages).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_missing() {
        let result = LlmClient::from_env("openrouter/test");
        let _ = result;
    }
}
