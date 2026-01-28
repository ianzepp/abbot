use openrouter_rs::{OpenRouterClient, api::chat::{ChatCompletionRequest, Message as ChatMessage}, types::Role};
use crate::bus::Message;

pub struct LlmClient {
    client: OpenRouterClient,
    model: String,
}

impl LlmClient {
    pub fn new(api_key: &str, model: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = OpenRouterClient::builder()
            .api_key(api_key)
            .build()?;

        Ok(Self {
            client,
            model: model.to_string(),
        })
    }

    pub fn from_env(model: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .map_err(|_| "OPENROUTER_API_KEY not set")?;
        Self::new(&api_key, model)
    }

    pub async fn chat(&self, system_prompt: &str, user_message: &str, history: &[Message]) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let mut messages = vec![
            ChatMessage::new(Role::System, system_prompt),
        ];

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

        let request = ChatCompletionRequest::builder()
            .model(&self.model)
            .messages(messages)
            .build()?;

        let response = self.client.send_chat_completion(&request).await?;

        let content = response.choices
            .first()
            .and_then(|c| c.content())
            .unwrap_or("(no response)")
            .to_string();

        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_missing() {
        let result = LlmClient::from_env("test");
        let _ = result;
    }
}
