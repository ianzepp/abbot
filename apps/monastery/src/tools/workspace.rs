use super::{Tool, ExecutionContext};

/// Tool for managing the monk's workspace layer (Layer 3).
///
/// Commands:
/// - `read` - Read the workspace for current channel
/// - `write\nCONTENT` - Replace the workspace for current channel
pub struct WorkspaceTool;

impl WorkspaceTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Tool for WorkspaceTool {
    fn name(&self) -> &str {
        "workspace"
    }

    fn description(&self) -> &str {
        "Manage workspace layer (read/write)"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let args = args.trim();

        if args == "read" {
            return self.read(ctx).await;
        }

        if args.starts_with("write") {
            // Content is everything after "write\n"
            let content = if args == "write" {
                ""
            } else if args.starts_with("write\n") {
                &args[6..]
            } else {
                return "usage: write\\nCONTENT".to_string();
            };
            return self.write(content, ctx).await;
        }

        "usage: read | write\\nCONTENT".to_string()
    }
}

impl WorkspaceTool {
    async fn read(&self, ctx: &ExecutionContext) -> String {
        let content = ctx.store.get_workspace(&ctx.sender, &ctx.channel).unwrap_or_default();

        if content.is_empty() {
            "(empty)".to_string()
        } else {
            content
        }
    }

    async fn write(&self, content: &str, ctx: &ExecutionContext) -> String {
        if let Err(e) = ctx.store.set_workspace(&ctx.sender, &ctx.channel, content) {
            return format!("error: {}", e);
        }

        tracing::debug!(
            monk = %ctx.sender,
            channel = %ctx.channel,
            len = content.len(),
            "wrote workspace layer"
        );
        format!("wrote {} bytes to workspace", content.len())
    }
}
