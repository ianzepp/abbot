use super::{Tool, ExecutionContext};
use std::path::Path;

pub struct CdTool;

#[async_trait::async_trait]
impl Tool for CdTool {
    fn name(&self) -> &str {
        "cd"
    }

    fn description(&self) -> &str {
        "Change working directory for this session"
    }

    async fn execute(&self, args: &str, _ctx: &ExecutionContext) -> String {
        let path = args.trim();
        if path.is_empty() {
            return "usage: cd <path>".to_string();
        }

        let target = Path::new(path);
        if !target.exists() {
            return format!("error: path does not exist: {}", path);
        }
        if !target.is_dir() {
            return format!("error: not a directory: {}", path);
        }

        match target.canonicalize() {
            Ok(canonical) => canonical.display().to_string(),
            Err(e) => format!("error: {}", e),
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
    async fn test_cd_empty() {
        let tool = CdTool;
        let result = tool.execute("", &test_ctx()).await;
        assert!(result.contains("usage"));
    }

    #[tokio::test]
    async fn test_cd_valid_dir() {
        let tool = CdTool;
        let result = tool.execute("/tmp", &test_ctx()).await;
        assert!(result.contains("tmp"));
        assert!(!result.contains("error"));
    }

    #[tokio::test]
    async fn test_cd_nonexistent() {
        let tool = CdTool;
        let result = tool.execute("/nonexistent/path", &test_ctx()).await;
        assert!(result.contains("error"));
    }
}
