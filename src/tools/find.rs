use super::{Tool, ExecutionContext};
use tokio::process::Command;

pub struct FindTool;

#[async_trait::async_trait]
impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "Find files by name pattern (e.g. !find *.rs)"
    }

    async fn execute(&self, args: &str, _ctx: &ExecutionContext) -> String {
        let args = args.trim();
        if args.is_empty() {
            return "usage: !find <pattern> [path]".to_string();
        }

        let mut parts = args.split_whitespace();
        let pattern = parts.next().unwrap();
        let path = parts.next().unwrap_or(".");

        match Command::new("find")
            .arg(path)
            .arg("-name")
            .arg(pattern)
            .arg("-type")
            .arg("f")
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if !stderr.is_empty() && stdout.is_empty() {
                    return format!("error: {}", stderr.trim());
                }

                let lines: Vec<&str> = stdout.lines().take(20).collect();
                if lines.is_empty() {
                    "no files found".to_string()
                } else {
                    let mut result = lines.join("\n");
                    let total = stdout.lines().count();
                    if total > 20 {
                        result.push_str(&format!("\n... and {} more", total - 20));
                    }
                    result
                }
            }
            Err(e) => format!("error: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_utils::test_context;

    fn test_ctx() -> ExecutionContext {
        test_context("/tmp")
    }

    #[tokio::test]
    async fn test_find_empty() {
        let tool = FindTool;
        let result = tool.execute("", &test_ctx()).await;
        assert!(result.contains("usage"));
    }

    #[tokio::test]
    async fn test_find_cargo() {
        let tool = FindTool;
        let result = tool.execute("Cargo.toml .", &test_ctx()).await;
        assert!(result.contains("Cargo.toml"));
    }
}
