use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use crate::bus::{Hub, Message, MessageOp, MessageData, respond};
use crate::chat::Client;
use super::{Dispatcher, ExecutionContext};
use super::validator::{Validator, ValidationContext, ValidationResult, AllowAll};

pub struct ToolAgent {
    dispatcher: Dispatcher,
    validator: Box<dyn Validator>,
    cwd_map: Mutex<HashMap<(String, String), PathBuf>>,
}

impl ToolAgent {
    pub fn new(dispatcher: Dispatcher) -> Self {
        Self {
            dispatcher,
            validator: Box::new(AllowAll),
            cwd_map: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_validator(mut self, validator: Box<dyn Validator>) -> Self {
        self.validator = validator;
        self
    }

    async fn get_cwd(&self, channel: &str, sender: &str) -> PathBuf {
        let key = (channel.to_string(), sender.to_string());
        let map = self.cwd_map.lock().await;
        map.get(&key)
            .cloned()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
    }

    async fn set_cwd(&self, channel: &str, sender: &str, path: PathBuf) {
        let key = (channel.to_string(), sender.to_string());
        let mut map = self.cwd_map.lock().await;
        map.insert(key, path);
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

        // Validate the request
        let validation_ctx = ValidationContext {
            tool,
            args,
            sender: &msg.sender,
            channel,
        };

        if let ValidationResult::Deny { code, message } = self.validator.validate(&validation_ctx) {
            tracing::warn!(tool, args, code = code.as_str(), "validation denied");
            let err = respond::error(self.name(), channel, code, message).with_reply_to(msg.id);
            client.publish(err).await;
            return;
        }

        if tool == "help" {
            let help = self.dispatcher.help();
            let response = respond::ok_text(self.name(), channel, help).with_reply_to(msg.id);
            client.publish(response).await;
            return;
        }

        let cwd = self.get_cwd(channel, &msg.sender).await;
        let exec_ctx = ExecutionContext {
            cwd: cwd.clone(),
            sender: msg.sender.clone(),
            channel: channel.to_string(),
        };

        // Handle cd specially: update cwd map on success
        if tool == "cd" {
            let result = self.dispatcher.execute(tool, args, &exec_ctx).await;
            match result {
                Some(output) => {
                    if !output.starts_with("error") && !output.starts_with("usage") {
                        self.set_cwd(channel, &msg.sender, PathBuf::from(&output)).await;
                    }
                    let item = respond::item_text(self.name(), channel, &output).with_reply_to(msg.id);
                    client.publish(item).await;
                    let done = respond::ok_text(self.name(), channel, "").with_reply_to(msg.id);
                    client.publish(done).await;
                }
                None => {
                    let err = respond::error(self.name(), channel, "ENOENT", format!("unknown tool: {}", tool)).with_reply_to(msg.id);
                    client.publish(err).await;
                }
            }
            return;
        }

        match self.dispatcher.execute(tool, args, &exec_ctx).await {
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
