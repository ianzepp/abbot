use super::{Tool, ExecutionContext};

/// Tool for managing the monk's self layer (Layer 2).
///
/// Commands:
/// - `read` - Read the self layer content
/// - `write\nCONTENT` - Replace the self layer
/// - `meditate` - Trigger compaction/reflection (future)
pub struct SelfTool;

impl SelfTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Tool for SelfTool {
    fn name(&self) -> &str {
        "self"
    }

    fn description(&self) -> &str {
        "Manage identity layer (read/write/meditate)"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let args = args.trim();

        if args == "read" {
            return self.read(ctx).await;
        }

        if args == "meditate" {
            return self.meditate(ctx).await;
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

        "usage: read | write\\nCONTENT | meditate".to_string()
    }
}

impl SelfTool {
    async fn read(&self, ctx: &ExecutionContext) -> String {
        let content = ctx.store.get_monk_self(&ctx.sender).unwrap_or_default();

        if content.is_empty() {
            "(empty)".to_string()
        } else {
            content
        }
    }

    async fn write(&self, content: &str, ctx: &ExecutionContext) -> String {
        if let Err(e) = ctx.store.set_monk_self(&ctx.sender, content) {
            return format!("error: {}", e);
        }

        tracing::debug!(monk = %ctx.sender, len = content.len(), "wrote self layer");
        format!("wrote {} bytes to self layer", content.len())
    }

    async fn meditate(&self, _ctx: &ExecutionContext) -> String {
        // Future: trigger compaction, call LLM to summarize/reflect
        "meditation not yet implemented".to_string()
    }
}
