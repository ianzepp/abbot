use super::{Tool, ExecutionContext};
use crate::llm::LlmClient;

const OPUS_MODEL: &str = "anthropic/claude-opus-4";

const WISDOM_PROMPT: &str = r#"You are the Rector of an AI monastery. A monk has come to you seeking guidance.

Your role:
- Provide clear, actionable direction
- Break down complex problems into steps
- Point out what they might be missing
- Suggest which tools or approaches to try
- Be direct and practical, not vague

You advise but do not act. The monk must do the work.

The monk has access to these tools: bash, read, write, edit, find, diff, monk, channel, self, workspace, garden.

Respond with concrete guidance they can act on immediately."#;

pub struct PrayTool;

impl PrayTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Tool for PrayTool {
    fn name(&self) -> &str {
        "pray"
    }

    fn description(&self) -> &str {
        "Seek guidance from Opus on complex problems"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let question = args.trim();
        if question.is_empty() {
            return "usage: pray <your question or context>".to_string();
        }

        // Build context for Opus
        let self_layer = ctx.store.get_monk_self(&ctx.sender).unwrap_or_default();
        let workspace = ctx.store.get_workspace(&ctx.sender, &ctx.channel).unwrap_or_default();

        let mut context_parts = vec![format!("**Question from {}:**\n{}", ctx.sender, question)];

        if !self_layer.is_empty() {
            context_parts.push(format!("**Monk's self-knowledge:**\n{}", self_layer));
        }

        if !workspace.is_empty() {
            context_parts.push(format!("**Current workspace ({}):**\n{}", ctx.channel, workspace));
        }

        let user_message = context_parts.join("\n\n");

        // Create Opus client
        let opus = match LlmClient::from_env(OPUS_MODEL) {
            Ok(client) => client,
            Err(e) => return format!("error: could not connect to Opus: {}", e),
        };

        // Seek wisdom
        match opus.chat(WISDOM_PROMPT, &user_message, &[]).await {
            Ok(response) => {
                tracing::info!(
                    monk = %ctx.sender,
                    question_len = question.len(),
                    response_len = response.len(),
                    "prayer answered"
                );
                response
            }
            Err(e) => format!("error: prayer failed: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_utils::test_context;

    #[tokio::test]
    async fn test_pray_empty() {
        let tool = PrayTool::new();
        let ctx = test_context("/tmp");
        let result = tool.execute("", &ctx).await;
        assert!(result.contains("usage"));
    }

    // Integration test - requires API key
    // #[tokio::test]
    // async fn test_pray_real() {
    //     let tool = PrayTool::new();
    //     let ctx = test_context("/tmp");
    //     let result = tool.execute("How should I approach refactoring a large function?", &ctx).await;
    //     assert!(!result.contains("error"), "got: {}", result);
    // }
}
