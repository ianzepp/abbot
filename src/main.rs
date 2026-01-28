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
use agent::{Agent, AgentContext, new_registry};
use chat::{Trait, render_traits, load_traits};
use irc::{Server, tool_notice};
use tools::{Dispatcher, ToolAgent, BashTool, CdTool, DiffTool, FindTool, LogsTool, MonkTool, PatchTool, PostTool, ReadTool, WriteTool};
use llm::LlmClient;
use history::{Store, HistoryAgent};

const HISTORY_CONTEXT_SIZE: usize = 100;
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TOOL_ITERATIONS: usize = 3;
const MAX_TOOL_OUTPUT_LINES: usize = 50;
const PING_INTERVAL_MIN: Duration = Duration::from_secs(30);
const PING_INTERVAL_MAX: Duration = Duration::from_secs(900); // 15 minutes

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

        tracing::debug!(%exec_id, tool, args, "exec_tool: starting");

        let rx = self.hub.read().await.subscribe(channel);
        self.hub.read().await.publish(channel, exec_msg);

        let mut rx = match rx {
            Some(r) => r,
            None => {
                tracing::error!("exec_tool: channel not found");
                return vec!["error: channel not found".to_string()];
            }
        };

        let mut results = Vec::new();
        let deadline = tokio::time::Instant::now() + TOOL_TIMEOUT;

        loop {
            if tokio::time::Instant::now() > deadline {
                tracing::warn!(%exec_id, "exec_tool: timeout");
                results.push("error: tool timeout".to_string());
                break;
            }

            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(msg)) => {
                    if msg.reply_to != Some(exec_id) {
                        continue;
                    }

                    tracing::debug!(%exec_id, op = ?msg.op, "exec_tool: received matching message");

                    match msg.op {
                        MessageOp::Item => {
                            if let Some(text) = msg.text() {
                                results.push(text.to_string());
                            }
                        }
                        MessageOp::Ok => {
                            tracing::debug!(%exec_id, result_count = results.len(), "exec_tool: done");
                            break;
                        }
                        MessageOp::Error => {
                            if let MessageData::Error { code, message } = &msg.data {
                                results.push(format!("error [{}]: {}", code, message));
                            }
                            break;
                        }
                        _ => {}
                    }
                }
                Ok(Err(e)) => {
                    tracing::error!(%exec_id, ?e, "exec_tool: channel error");
                    break;
                }
                Err(_) => continue,
            }
        }

        tracing::debug!(%exec_id, result_count = results.len(), "exec_tool: returning");
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
        let (content, is_ping) = match msg.op {
            MessageOp::Chat => {
                match msg.text() {
                    Some(t) => (t.to_string(), false),
                    None => return,
                }
            }
            MessageOp::Ping => ("<ping/>".to_string(), true),
            _ => return,
        };

        let Some(llm) = &self.llm else { return };

        let history = self.store.recent_chat(&msg.channel, HISTORY_CONTEXT_SIZE).unwrap_or_default();
        let system_prompt = self.system_prompt();

        let mut conversation = vec![content];
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
                    if !is_ping {
                        ctx.client.say(&msg.channel, &format!("error: {}", e)).await;
                    }
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
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    // Suppress "<pong/>" responses from pings
                    if is_ping && trimmed == "<pong/>" {
                        continue;
                    }
                    ctx.client.say(&msg.channel, line).await;
                }
                break;
            }

            conversation.push(format!("<assistant>\n{}\n</assistant>", response));
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

    // Agent registry for sub-agents
    let registry = new_registry();

    // Get API key for sub-agent spawning
    let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();

    // Tool agent - handles tool execution
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(BashTool));
    dispatcher.register(Box::new(CdTool));
    dispatcher.register(Box::new(DiffTool));
    dispatcher.register(Box::new(FindTool));
    dispatcher.register(Box::new(LogsTool::new(store.clone())));
    dispatcher.register(Box::new(MonkTool::new(hub.clone(), registry.clone(), api_key)));
    dispatcher.register(Box::new(PatchTool));
    dispatcher.register(Box::new(PostTool::new(hub.clone())));
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
    let traits = load_traits("traits", &["system", "tools", "ping"])
        .expect("failed to load traits");
    tracing::info!(count = traits.len(), "traits loaded");
    let chat_agent = ChatAgent::new(llm, store, traits, hub.clone());
    let hub_clone = hub.clone();
    tokio::spawn(async move {
        chat_agent.run(hub_clone).await;
    });

    // Heartbeat - pings with dynamic frequency based on user activity
    let hub_clone = hub.clone();
    tokio::spawn(async move {
        let mut rx = hub_clone.read().await.subscribe("#general").unwrap();
        let mut interval = PING_INTERVAL_MIN;
        let mut last_user_activity = std::time::Instant::now();

        loop {
            let deadline = tokio::time::Instant::now() + interval;

            // Drain messages, watching for user activity
            loop {
                let timeout = deadline.saturating_duration_since(tokio::time::Instant::now());
                if timeout.is_zero() {
                    break;
                }

                match tokio::time::timeout(timeout, rx.recv()).await {
                    Ok(Ok(msg)) => {
                        // User activity: Chat from non-bot sender
                        if msg.op == MessageOp::Chat && !msg.sender.starts_with('_') && msg.sender != "abbot" {
                            last_user_activity = std::time::Instant::now();
                            interval = PING_INTERVAL_MIN;
                            tracing::debug!(interval_secs = interval.as_secs(), "ping: user activity, reset interval");
                        }
                    }
                    Ok(Err(_)) => break, // Channel closed
                    Err(_) => break, // Timeout - time to ping
                }
            }

            // Send ping
            let ping = respond::ping("_heartbeat", "#general");
            hub_clone.read().await.publish("#general", ping);

            // If no user activity since last ping, double interval
            if last_user_activity.elapsed() > interval {
                let new_interval = (interval * 2).min(PING_INTERVAL_MAX);
                if new_interval != interval {
                    interval = new_interval;
                    tracing::debug!(interval_secs = interval.as_secs(), "ping: no activity, increased interval");
                }
            }
        }
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
