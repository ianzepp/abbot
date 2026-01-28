use std::sync::Arc;
use tokio::sync::RwLock;
use super::{Tool, ExecutionContext};
use crate::bus::{Hub, respond};

pub struct PostTool {
    hub: Arc<RwLock<Hub>>,
}

impl PostTool {
    pub fn new(hub: Arc<RwLock<Hub>>) -> Self {
        Self { hub }
    }
}

#[async_trait::async_trait]
impl Tool for PostTool {
    fn name(&self) -> &str {
        "post"
    }

    fn description(&self) -> &str {
        "Post message to channel (e.g. post #monk-abc123 do the task)"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let args = args.trim();

        // Parse: #channel message
        let (channel, message) = if args.starts_with('#') {
            match args.split_once(' ') {
                Some((ch, msg)) => (ch.trim(), msg.trim()),
                None => return "usage: post <#channel> <message>".to_string(),
            }
        } else {
            return "usage: post <#channel> <message>".to_string();
        };

        if message.is_empty() {
            return "usage: post <#channel> <message>".to_string();
        }

        // Check channel exists
        let exists = self.hub.read().await.subscribe(channel).is_some();
        if !exists {
            return format!("error: channel {} not found", channel);
        }

        // Post message from the sender (abbot)
        let msg = respond::chat(&ctx.sender, channel, message);
        self.hub.read().await.publish(channel, msg);

        format!("posted to {}", channel)
    }
}
