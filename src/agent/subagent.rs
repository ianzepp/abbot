use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, oneshot};
use crate::bus::{Hub, MessageOp, MessageData, respond};
use crate::chat::Client;
use crate::llm::LlmClient;
use crate::irc::tool_notice;

const MAX_TOOL_ITERATIONS: usize = 10;
const MAX_TOOL_OUTPUT_LINES: usize = 50;
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Monk {
    id: String,
    channel: String,
    llm: Arc<LlmClient>,
    system_prompt: String,
}

impl Monk {
    pub fn new(id: String, channel: String, llm: LlmClient) -> Self {
        let system_prompt = format!(
            "You are monk {}, a task-focused assistant in a monastery led by abbot. \
             Execute tasks given to you using available tools. \
             When done, provide a clear summary of what you accomplished. \
             Keep responses concise.",
            id
        );

        Self {
            id,
            channel,
            llm: Arc::new(llm),
            system_prompt,
        }
    }

    pub fn name(&self) -> String {
        format!("monk-{}", self.id)
    }

    fn parse_exec_tags(text: &str) -> (Vec<(String, String)>, String) {
        let mut tools = Vec::new();
        let mut remaining = text.to_string();

        while let Some(start) = remaining.find("<exec tool=\"") {
            let tool_start = start + 12;
            let Some(tool_end) = remaining[tool_start..].find("\">") else { break };
            let tool = remaining[tool_start..tool_start + tool_end].to_string();

            let args_start = tool_start + tool_end + 2;
            let Some(end_tag) = remaining[args_start..].find("</exec>") else { break };
            let args = remaining[args_start..args_start + end_tag].trim().to_string();

            tools.push((tool, args));

            remaining = format!(
                "{}{}",
                &remaining[..start],
                &remaining[args_start + end_tag + 7..]
            );
        }

        (tools, remaining)
    }

    async fn exec_tool(&self, hub: &Arc<RwLock<Hub>>, tool: &str, args: &str) -> Vec<String> {
        let exec_msg = respond::exec(&self.name(), &self.channel, tool, args);
        let exec_id = exec_msg.id;

        let rx = hub.read().await.subscribe(&self.channel);
        hub.read().await.publish(&self.channel, exec_msg);

        let mut rx = match rx {
            Some(r) => r,
            None => return vec!["error: channel not found".to_string()],
        };

        let mut results = Vec::new();
        let deadline = tokio::time::Instant::now() + TOOL_TIMEOUT;

        loop {
            if tokio::time::Instant::now() > deadline {
                results.push("error: tool timeout".to_string());
                break;
            }

            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(msg)) => {
                    if msg.reply_to != Some(exec_id) {
                        continue;
                    }

                    match msg.op {
                        MessageOp::Item => {
                            if let Some(text) = msg.text() {
                                results.push(text.to_string());
                            }
                        }
                        MessageOp::Ok => break,
                        MessageOp::Error => {
                            if let MessageData::Error { code, message } = &msg.data {
                                results.push(format!("error [{}]: {}", code, message));
                            }
                            break;
                        }
                        _ => {}
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }

        results
    }

    fn notify_general(&self, hub: &Arc<RwLock<Hub>>, message: &str) {
        let msg = respond::chat(&self.name(), "#general", message);
        // Fire and forget - don't await
        let hub = hub.clone();
        let msg = msg;
        tokio::spawn(async move {
            hub.read().await.publish("#general", msg);
        });
    }

    pub async fn run(self, hub: Arc<RwLock<Hub>>, mut shutdown: oneshot::Receiver<()>) {
        let client = Client::new(&self.name(), hub.clone());

        let mut rx = match client.join(&self.channel).await {
            Some(r) => r,
            None => {
                tracing::error!(monk = self.name(), "failed to join channel");
                return;
            }
        };

        tracing::info!(monk = self.name(), channel = self.channel, "monk started");

        let mut conversation: Vec<String> = Vec::new();

        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    tracing::info!(monk = self.name(), "shutting down");
                    break;
                }
                result = rx.recv() => {
                    match result {
                        Ok(msg) => {
                            // Only respond to Chat messages from others
                            if msg.op != MessageOp::Chat || msg.sender == self.name() {
                                continue;
                            }

                            let content = match msg.text() {
                                Some(t) => t.to_string(),
                                None => continue,
                            };

                            tracing::debug!(monk = self.name(), from = msg.sender, "received instruction");

                            // Add to conversation
                            conversation.push(format!("<instruction from=\"{}\">\n{}\n</instruction>", msg.sender, content));

                            // Process with LLM
                            let completed = self.process_instruction(&hub, &client, &mut conversation).await;

                            // Notify #general when task completes
                            if completed {
                                self.notify_general(&hub, &format!("Task complete in {}", self.channel));
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(monk = self.name(), skipped = n, "lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::info!(monk = self.name(), "channel closed");
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn process_instruction(&self, hub: &Arc<RwLock<Hub>>, client: &Client, conversation: &mut Vec<String>) -> bool {
        let mut iterations = 0;

        loop {
            iterations += 1;
            if iterations > MAX_TOOL_ITERATIONS {
                client.say(&self.channel, "(tool limit reached)").await;
                return false;
            }

            let combined_input = conversation.join("\n");

            let response = match self.llm.chat(&self.system_prompt, &combined_input, &[]).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(monk = self.name(), ?e, "LLM error");
                    client.say(&self.channel, &format!("error: {}", e)).await;
                    return false;
                }
            };

            let (tools, remaining_text) = Self::parse_exec_tags(&response);

            let mut tool_results = Vec::new();
            for (tool, args) in &tools {
                client.say(&self.channel, &tool_notice(tool, args)).await;
                tracing::debug!(monk = self.name(), tool, args, "executing tool");

                let results = self.exec_tool(hub, tool, args).await;
                let mut result_lines: Vec<_> = results.into_iter().take(MAX_TOOL_OUTPUT_LINES).collect();
                if result_lines.len() == MAX_TOOL_OUTPUT_LINES {
                    result_lines.push("(output truncated)".to_string());
                }
                let result_text = result_lines.join("\n");
                tool_results.push(format!("<tool name=\"{}\" args=\"{}\">\n{}\n</tool>", tool, args, result_text));
            }

            if tools.is_empty() {
                // No tools - this is the final response
                for line in remaining_text.lines() {
                    if !line.trim().is_empty() {
                        client.say(&self.channel, line).await;
                    }
                }
                // Add response to conversation for context
                conversation.push(format!("<response>\n{}\n</response>", remaining_text));
                return true; // Task completed
            }

            conversation.push(format!("<assistant>\n{}\n</assistant>", response));
            conversation.push(format!("<tool-results>\n{}\n</tool-results>", tool_results.join("\n")));
        }
    }
}
