use std::sync::Arc;
use super::{Tool, ExecutionContext};
use crate::history::Store;

pub struct LogsTool {
    store: Arc<Store>,
}

impl LogsTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    fn format_message(msg: &crate::bus::Message) -> String {
        let time = msg.timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| {
                let secs = d.as_secs();
                let hours = (secs / 3600) % 24;
                let mins = (secs / 60) % 60;
                format!("{:02}:{:02}", hours, mins)
            })
            .unwrap_or_else(|_| "??:??".to_string());

        let content = msg.text().unwrap_or("(no text)");
        format!("[{}] <{}> {}", time, msg.sender, content)
    }
}

#[async_trait::async_trait]
impl Tool for LogsTool {
    fn name(&self) -> &str {
        "logs"
    }

    fn description(&self) -> &str {
        "Query message history (e.g. !logs 20, !logs search foo, !logs from alice)"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let parts: Vec<&str> = args.trim().split_whitespace().collect();
        let channel = &ctx.channel;

        match parts.as_slice() {
            [] => {
                // Default: last 10 messages
                match self.store.recent_chat(channel, 10) {
                    Ok(msgs) => msgs.iter().map(Self::format_message).collect::<Vec<_>>().join("\n"),
                    Err(e) => format!("error: {}", e),
                }
            }
            [count] if count.parse::<usize>().is_ok() => {
                let n: usize = count.parse().unwrap();
                match self.store.recent_chat(channel, n.min(100)) {
                    Ok(msgs) => msgs.iter().map(Self::format_message).collect::<Vec<_>>().join("\n"),
                    Err(e) => format!("error: {}", e),
                }
            }
            ["search", rest @ ..] if !rest.is_empty() => {
                let query = rest.join(" ");
                match self.store.search(channel, &query, 20) {
                    Ok(msgs) if msgs.is_empty() => "no matches found".to_string(),
                    Ok(msgs) => msgs.iter().map(Self::format_message).collect::<Vec<_>>().join("\n"),
                    Err(e) => format!("error: {}", e),
                }
            }
            ["from", sender] => {
                match self.store.recent_from(channel, sender, 20) {
                    Ok(msgs) if msgs.is_empty() => format!("no messages from {}", sender),
                    Ok(msgs) => msgs.iter().map(Self::format_message).collect::<Vec<_>>().join("\n"),
                    Err(e) => format!("error: {}", e),
                }
            }
            ["all", count] if count.parse::<usize>().is_ok() => {
                // All message types, not just Chat
                let n: usize = count.parse().unwrap();
                match self.store.recent(channel, n.min(100)) {
                    Ok(msgs) => msgs.iter().map(Self::format_message).collect::<Vec<_>>().join("\n"),
                    Err(e) => format!("error: {}", e),
                }
            }
            _ => "usage: logs [count], logs search <term>, logs from <sender>, logs all <count>".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::bus::respond;

    fn test_ctx() -> ExecutionContext {
        ExecutionContext {
            cwd: PathBuf::from("/tmp"),
            sender: "test".to_string(),
            channel: "#test".to_string(),
        }
    }

    #[tokio::test]
    async fn test_logs_empty() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let tool = LogsTool::new(store);
        let result = tool.execute("", &test_ctx()).await;
        assert!(result.is_empty() || result.contains("error"));
    }

    #[tokio::test]
    async fn test_logs_with_messages() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        let msg = respond::chat("alice", "#test", "hello world");
        store.insert(&msg).unwrap();

        let tool = LogsTool::new(store);
        let result = tool.execute("", &test_ctx()).await;
        assert!(result.contains("alice"));
        assert!(result.contains("hello world"));
    }

    #[tokio::test]
    async fn test_logs_search() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        store.insert(&respond::chat("alice", "#test", "hello world")).unwrap();
        store.insert(&respond::chat("bob", "#test", "goodbye world")).unwrap();

        let tool = LogsTool::new(store);
        let result = tool.execute("search hello", &test_ctx()).await;
        assert!(result.contains("alice"));
        assert!(!result.contains("bob"));
    }
}
