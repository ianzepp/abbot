use std::sync::Arc;
use tokio::sync::RwLock;
use crate::bus::{Hub, Message, MessageOp, MessageData, respond};
use crate::chat::Client;
use super::Dispatcher;

pub struct ToolAgent {
    dispatcher: Dispatcher,
}

impl ToolAgent {
    pub fn new(dispatcher: Dispatcher) -> Self {
        Self { dispatcher }
    }

    pub fn name(&self) -> &str {
        "tools"
    }

    pub fn channels(&self) -> Vec<&str> {
        vec!["#general"]
    }

    pub async fn run(&self, hub: Arc<RwLock<Hub>>) {
        let client = Client::new(self.name(), hub.clone());

        let mut receivers = Vec::new();
        for channel in self.channels() {
            if let Some(rx) = client.join(channel).await {
                receivers.push((channel.to_string(), rx));
            }
        }

        tracing::info!(agent = self.name(), "started");

        loop {
            for (channel, rx) in &mut receivers {
                match rx.try_recv() {
                    Ok(msg) => {
                        if msg.sender != self.name() && msg.op == MessageOp::Exec {
                            self.handle_exec(&client, channel, msg).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                        tracing::warn!(agent = self.name(), skipped = n, "lagged");
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                        tracing::error!(agent = self.name(), "channel closed");
                        return;
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }

    async fn handle_exec(&self, client: &Client, channel: &str, msg: Message) {
        let (tool, args) = match &msg.data {
            MessageData::Exec { tool, args } => (tool.as_str(), args.as_str()),
            _ => return,
        };

        tracing::debug!(tool, args, "executing");

        if tool == "help" {
            let help = self.dispatcher.help();
            let response = respond::ok_text(self.name(), channel, help).with_reply_to(msg.id);
            client.publish(response).await;
            return;
        }

        match self.dispatcher.execute(tool, args).await {
            Some(result) => {
                for line in result.lines() {
                    if !line.is_empty() {
                        let item = respond::item_text(self.name(), channel, line).with_reply_to(msg.id);
                        client.publish(item).await;
                    }
                }
                let done = respond::ok_text(self.name(), channel, "").with_reply_to(msg.id);
                client.publish(done).await;
            }
            None => {
                let err = respond::error(self.name(), channel, "ENOENT", format!("unknown tool: {}", tool)).with_reply_to(msg.id);
                client.publish(err).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_agent_handles_exec() {
        let hub = Arc::new(RwLock::new(Hub::new()));
        hub.write().await.create_channel("#test");

        let mut dispatcher = Dispatcher::new();
        use crate::tools::BashTool;
        dispatcher.register(Box::new(BashTool));

        let agent = ToolAgent::new(dispatcher);

        let hub_clone = hub.clone();
        let handle = tokio::spawn(async move {
            agent.run(hub_clone).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let msg = respond::exec("user", "#test", "bash", "echo hello");
        hub.read().await.publish("#test", msg);

        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        handle.abort();
    }
}
