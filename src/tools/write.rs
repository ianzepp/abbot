use super::{Tool, ExecutionContext};
use tokio::fs;

pub struct WriteTool;

#[async_trait::async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write file: write <file> <content>"
    }

    async fn execute(&self, args: &str, _ctx: &ExecutionContext) -> String {
        let args = args.trim();
        if args.is_empty() {
            return "usage: write <file> <content>".to_string();
        }

        let mut parts = args.splitn(2, ' ');
        let path = match parts.next() {
            Some(p) => p,
            None => return "error: no file specified".to_string(),
        };

        let content = match parts.next() {
            Some(c) => c,
            None => return "error: no content specified".to_string(),
        };

        match fs::write(path, content).await {
            Ok(_) => format!("wrote {} bytes to {}", content.len(), path),
            Err(e) => format!("error writing file: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_ctx() -> ExecutionContext {
        ExecutionContext {
            cwd: PathBuf::from("/tmp"),
            sender: "test".to_string(),
            channel: "#test".to_string(),
        }
    }

    #[tokio::test]
    async fn test_write_empty() {
        let tool = WriteTool;
        let result = tool.execute("", &test_ctx()).await;
        assert!(result.contains("usage"));
    }
}
