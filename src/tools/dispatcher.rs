// Dispatcher routes tool execution requests to the appropriate tool implementation.
//
// Tools are registered by name and invoked via the !toolname syntax. The dispatcher
// parses tool calls, looks up the implementation, and executes it with the provided
// context. Help is synthesized from registered tool descriptions.

use std::collections::HashMap;
use super::{Tool, ExecutionContext};

pub struct Dispatcher {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl Dispatcher {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn parse(message: &str) -> Option<(&str, &str)> {
        let message = message.trim();
        if !message.starts_with('!') {
            return None;
        }

        let without_bang = &message[1..];
        let mut parts = without_bang.splitn(2, ' ');
        let cmd = parts.next()?;
        let args = parts.next().unwrap_or("");

        Some((cmd, args))
    }

    pub async fn dispatch(&self, message: &str, ctx: &ExecutionContext) -> Option<String> {
        let (cmd, args) = Self::parse(message)?;

        if cmd == "help" {
            return Some(self.help());
        }

        if let Some(tool) = self.tools.get(cmd) {
            Some(tool.execute(args, ctx).await)
        } else {
            Some(format!("unknown command: !{}", cmd))
        }
    }

    pub async fn execute(&self, tool: &str, args: &str, ctx: &ExecutionContext) -> Option<String> {
        if let Some(t) = self.tools.get(tool) {
            Some(t.execute(args, ctx).await)
        } else {
            None
        }
    }

    pub fn help(&self) -> String {
        let mut lines = vec!["commands:".to_string()];
        for (name, tool) in &self.tools {
            lines.push(format!("  !{} - {}", name, tool.description()));
        }
        lines.push("  !help - show this help".to_string());
        lines.join("\n")
    }
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command() {
        assert_eq!(Dispatcher::parse("!bash ls"), Some(("bash", "ls")));
        assert_eq!(Dispatcher::parse("!bash ls -la"), Some(("bash", "ls -la")));
        assert_eq!(Dispatcher::parse("!help"), Some(("help", "")));
        assert_eq!(Dispatcher::parse("hello"), None);
        assert_eq!(Dispatcher::parse(""), None);
    }
}
