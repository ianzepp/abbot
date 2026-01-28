use openrouter_rs::{OpenRouterClient, api::chat::*, types::Role};
use crate::history::HistoryMessage;

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

    pub async fn chat(&self, user_message: &str, history: &[HistoryMessage]) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let mut messages = vec![
            Message::new(Role::System, r#"You are abbot, a helpful IRC bot running in #general. Keep responses concise (1-3 lines) since this is IRC.

You have tools available via ! commands:
- !bash <cmd> - execute shell commands
- !find <pattern> [path] - find files by name
- !read <file> - read file contents
- !edit <file> s/old/new/ - substitute text in file
- !edit <file> append <text> - append to file
- !diff [file] - show git diff or compare files
- !help - list all commands

When users ask about capabilities, mention these tools. You cannot execute tools directly - users must type the ! commands themselves."#),
        ];

        for msg in history {
            let role = if msg.sender == "abbot" {
                Role::Assistant
            } else {
                Role::User
            };
            let content = if role == Role::User {
                format!("<{}> {}", msg.sender, msg.content)
            } else {
                msg.content.clone()
            };
            messages.push(Message::new(role, content));
        }

        messages.push(Message::new(Role::User, user_message));

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
