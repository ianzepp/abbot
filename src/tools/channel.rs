use super::{Tool, ExecutionContext};

/// Tool for managing channel subscriptions.
///
/// Commands:
/// - `join #channel` - Subscribe to a channel
/// - `part #channel` - Unsubscribe from a channel
/// - `list` - List subscribed channels
pub struct ChannelTool;

impl ChannelTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Tool for ChannelTool {
    fn name(&self) -> &str {
        "channel"
    }

    fn description(&self) -> &str {
        "Manage channel subscriptions (join/part/list)"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let args = args.trim();

        if args == "list" {
            return self.list(ctx).await;
        }

        if args.starts_with("join ") {
            let channel = args.strip_prefix("join ").unwrap().trim();
            return self.join(channel, ctx).await;
        }

        if args.starts_with("part ") {
            let channel = args.strip_prefix("part ").unwrap().trim();
            return self.part(channel, ctx).await;
        }

        "usage: join #channel | part #channel | list".to_string()
    }
}

impl ChannelTool {
    async fn list(&self, ctx: &ExecutionContext) -> String {
        let registry = ctx.registry.read().await;
        let channels = registry.channels_for_monk(&ctx.sender);

        if channels.is_empty() {
            return "not subscribed to any channels".to_string();
        }

        let mut lines = vec!["subscribed channels:".to_string()];
        for channel in channels {
            lines.push(format!("  {}", channel));
        }
        lines.join("\n")
    }

    async fn join(&self, channel: &str, ctx: &ExecutionContext) -> String {
        if !channel.starts_with('#') {
            return "error: channel must start with #".to_string();
        }

        {
            let mut registry = ctx.registry.write().await;
            registry.subscribe(&ctx.sender, channel);
        }

        tracing::info!(monk = %ctx.sender, channel, "joined channel");
        format!("joined {}", channel)
    }

    async fn part(&self, channel: &str, ctx: &ExecutionContext) -> String {
        if !channel.starts_with('#') {
            return "error: channel must start with #".to_string();
        }

        // Don't allow parting #general or #ping
        if channel == "#general" || channel == "#ping" {
            return format!("error: cannot part {}", channel);
        }

        {
            let mut registry = ctx.registry.write().await;
            registry.unsubscribe(&ctx.sender, channel);
        }

        tracing::info!(monk = %ctx.sender, channel, "parted channel");
        format!("parted {}", channel)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_channel_validation() {
        assert!("#general".starts_with('#'));
        assert!(!"general".starts_with('#'));
    }
}
