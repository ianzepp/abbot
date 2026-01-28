use super::Tool;
use tokio::fs;

pub struct ReadTool;

#[async_trait::async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read file contents (e.g. !read src/main.rs)"
    }

    async fn execute(&self, args: &str) -> String {
        let path = args.trim();
        if path.is_empty() {
            return "usage: !read <file>".to_string();
        }

        match fs::read_to_string(path).await {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().take(30).collect();
                let mut result = lines.join("\n");
                let total = content.lines().count();
                if total > 30 {
                    result.push_str(&format!("\n... ({} more lines)", total - 30));
                }
                result
            }
            Err(e) => format!("error: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_empty() {
        let tool = ReadTool;
        let result = tool.execute("").await;
        assert!(result.contains("usage"));
    }

    #[tokio::test]
    async fn test_read_cargo() {
        let tool = ReadTool;
        let result = tool.execute("Cargo.toml").await;
        assert!(result.contains("[package]"));
    }

    #[tokio::test]
    async fn test_read_nonexistent() {
        let tool = ReadTool;
        let result = tool.execute("/nonexistent/file").await;
        assert!(result.contains("error"));
    }
}
