mod bus;
mod chat;
mod agent;
mod irc;
mod tools;
mod llm;
mod history;

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use bus::{Hub, Message, MessageOp, MessageData, respond};
use agent::{Agent, AgentContext};
use chat::{Trait, render_traits, load_traits};
use irc::{Server, tool_notice};
use tools::{Dispatcher, ToolAgent, BashTool, DiffTool, EditTool, FindTool, ReadTool, WriteTool};
use llm::LlmClient;
use history::{Store, HistoryAgent};

const HISTORY_CONTEXT_SIZE: usize = 20;
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TOOL_ITERATIONS: usize = 3;
const MAX_TOOL_OUTPUT_LINES: usize = 50;

struct ToolRequest {
    tool: String,
    args: String,
}

struct ChatAgent {
    llm: Option<Arc<LlmClient>>,
    store: Arc<Store>,
    traits: Vec<Trait>,
    hub: Arc<RwLock<Hub>>,
}

impl ChatAgent {
    fn new(llm: Option<LlmClient>, store: Arc<Store>, traits: Vec<Trait>, hub: Arc<RwLock<Hub>>) -> Self {
        Self {
            llm: llm.map(Arc::new),
            store,
            traits,
            hub,
        }
    }

    fn system_prompt(&self) -> String {
        render_traits(&self.traits)
    }

    fn parse_exec_tags(text: &str) -> (Vec<ToolRequest>, String) {
        let mut tools = Vec::new();
        let mut remaining = text.to_string();

        while let Some(start) = remaining.find("<exec tool=\"") {
            let tool_start = start + 12;
            let Some(tool_end) = remaining[tool_start..].find("\">") else { break };
            let tool = remaining[tool_start..tool_start + tool_end].to_string();

            let args_start = tool_start + tool_end + 2;
            let Some(end_tag) = remaining[args_start..].find("</exec>") else { break };
            let args = remaining[args_start..args_start + end_tag].trim().to_string();

            tools.push(ToolRequest { tool, args });

            remaining = format!(
                "{}{}",
                &remaining[..start],
                &remaining[args_start + end_tag + 7..]
            );
        }

        (tools, remaining)
    }

    async fn exec_tool(&self, channel: &str, tool: &str, args: &str) -> Vec<String> {
        let exec_msg = respond::exec("abbot", channel, tool, args);
        let exec_id = exec_msg.id;

        let rx = self.hub.read().await.subscribe(channel);
        self.hub.read().await.publish(channel, exec_msg);

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
}

impl Agent for ChatAgent {
    fn name(&self) -> &str {
        "abbot"
    }

    fn channels(&self) -> Vec<&str> {
        vec!["#general"]
    }

    async fn on_message(&self, ctx: &AgentContext, msg: Message) {
        if msg.op != MessageOp::Chat {
            return;
        }

        let content = match msg.text() {
            Some(t) => t,
            None => return,
        };

        let Some(llm) = &self.llm else { return };

        let history = self.store.recent_chat(&msg.channel, HISTORY_CONTEXT_SIZE).unwrap_or_default();
        let system_prompt = self.system_prompt();

        let mut conversation = vec![content.to_string()];
        let mut iterations = 0;

        loop {
            iterations += 1;
            if iterations > MAX_TOOL_ITERATIONS {
                ctx.client.say(&msg.channel, "(tool limit reached)").await;
                break;
            }
            let combined_input = conversation.join("\n");

            let response = match llm.chat(&system_prompt, &combined_input, &history).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(?e, "LLM error");
                    ctx.client.say(&msg.channel, &format!("error: {}", e)).await;
                    return;
                }
            };

            let (tools, remaining_text) = Self::parse_exec_tags(&response);

            let mut tool_results = Vec::new();
            for req in &tools {
                ctx.client.say(&msg.channel, &tool_notice(&req.tool, &req.args)).await;
                tracing::debug!(tool = req.tool, args = req.args, "executing tool");

                let results = self.exec_tool(&msg.channel, &req.tool, &req.args).await;
                let mut result_lines: Vec<_> = results.into_iter().take(MAX_TOOL_OUTPUT_LINES).collect();
                if result_lines.len() == MAX_TOOL_OUTPUT_LINES {
                    result_lines.push("(output truncated)".to_string());
                }
                let result_text = result_lines.join("\n");
                tool_results.push(format!("<tool name=\"{}\" args=\"{}\">\n{}\n</tool>", req.tool, req.args, result_text));
            }

            if tools.is_empty() {
                for line in remaining_text.lines() {
                    if !line.trim().is_empty() {
                        ctx.client.say(&msg.channel, line).await;
                    }
                }
                break;
            }

            conversation.push(format!("<tool-results>\n{}\n</tool-results>", tool_results.join("\n")));
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let store = Arc::new(Store::open("abbot.db").expect("failed to open database"));
    tracing::info!("database opened: abbot.db");

    let hub = Arc::new(RwLock::new(Hub::new()));
    hub.write().await.create_channel("#general");

    // History agent - records all messages
    let history_agent = HistoryAgent::new(store.clone());
    let hub_clone = hub.clone();
    tokio::spawn(async move {
        history_agent.run(hub_clone).await;
    });

    // Tool agent - handles tool execution
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(BashTool));
    dispatcher.register(Box::new(DiffTool));
    dispatcher.register(Box::new(EditTool));
    dispatcher.register(Box::new(FindTool));
    dispatcher.register(Box::new(ReadTool));
    dispatcher.register(Box::new(WriteTool));

    let tool_agent = ToolAgent::new(dispatcher);
    let hub_clone = hub.clone();
    tokio::spawn(async move {
        tool_agent.run(hub_clone).await;
    });

    // LLM
    let llm = match LlmClient::from_env("anthropic/claude-sonnet-4") {
        Ok(client) => {
            tracing::info!("LLM enabled");
            Some(client)
        }
        Err(e) => {
            tracing::warn!(?e, "LLM disabled");
            None
        }
    };

    // Chat agent - handles LLM conversations
    let traits = load_traits("traits", &["system", "tools"])
        .expect("failed to load traits");
    tracing::info!(count = traits.len(), "traits loaded");
    let chat_agent = ChatAgent::new(llm, store, traits, hub.clone());
    let hub_clone = hub.clone();
    tokio::spawn(async move {
        chat_agent.run(hub_clone).await;
    });

    let server = Server::new(hub.clone(), 6667);
    tracing::info!("starting abbot");

    if let Err(e) = server.run().await {
        tracing::error!(?e, "IRC server error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_exec_single() {
        let text = r#"<exec tool="bash">ls -la</exec>"#;
        let (tools, remaining) = ChatAgent::parse_exec_tags(text);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool, "bash");
        assert_eq!(tools[0].args, "ls -la");
        assert!(remaining.trim().is_empty());
    }

    #[test]
    fn test_parse_exec_multiline() {
        let text = r#"<exec tool="bash">
cat << 'EOF' > test.txt
hello
world
EOF
</exec>"#;
        let (tools, remaining) = ChatAgent::parse_exec_tags(text);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool, "bash");
        assert!(tools[0].args.contains("hello"));
        assert!(tools[0].args.contains("world"));
        assert!(remaining.trim().is_empty());
    }

    #[test]
    fn test_parse_exec_multiple() {
        let text = r#"<exec tool="bash">ls</exec>
<exec tool="read">Cargo.toml</exec>"#;
        let (tools, remaining) = ChatAgent::parse_exec_tags(text);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].tool, "bash");
        assert_eq!(tools[1].tool, "read");
        assert!(remaining.trim().is_empty());
    }

    #[test]
    fn test_parse_exec_with_text() {
        let text = r#"Let me check that for you.
<exec tool="bash">ls</exec>
Here's what I found."#;
        let (tools, remaining) = ChatAgent::parse_exec_tags(text);
        assert_eq!(tools.len(), 1);
        assert!(remaining.contains("Let me check"));
        assert!(remaining.contains("Here's what I found"));
    }
}
