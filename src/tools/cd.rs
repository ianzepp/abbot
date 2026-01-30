// CdTool changes the shared working directory for a task session.
//
// Unlike system cd, this modifies the ExecutionContext's shared cwd, affecting
// subsequent tool calls in the same task. The directory is canonicalized to
// avoid symlink issues. Running cd with no arguments prints the current directory.

use super::{Tool, ExecutionContext};
use std::path::{Path, PathBuf};

pub struct CdTool;

#[async_trait::async_trait]
impl Tool for CdTool {
    fn name(&self) -> &str {
        "cd"
    }

    fn description(&self) -> &str {
        "Change working directory for this session"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let path = args.trim();
        if path.is_empty() {
            let cwd = ctx.cwd.lock().unwrap();
            return cwd.display().to_string();
        }

        // Get current directory for resolving relative paths
        let current = ctx.cwd.lock().unwrap().clone();

        // Resolve the target path (handles relative and absolute)
        let target: PathBuf = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            current.join(path)
        };

        if !target.exists() {
            return format!("error: path does not exist: {}", path);
        }
        if !target.is_dir() {
            return format!("error: not a directory: {}", path);
        }

        match target.canonicalize() {
            Ok(canonical) => {
                let mut cwd = ctx.cwd.lock().unwrap();
                *cwd = canonical.clone();
                canonical.display().to_string()
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
    async fn test_cd_empty() {
        let tool = CdTool;
        let result = tool.execute("", &test_ctx()).await;
        // cd with no args prints current directory
        assert!(result.contains("tmp"));
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

    #[tokio::test]
    async fn test_cd_modifies_cwd() {
        let tool = CdTool;
        let ctx = test_context("/tmp");

        // cd to /var
        let result = tool.execute("/var", &ctx).await;
        assert!(result.contains("var"));

        // Verify the cwd was actually changed
        let new_cwd = ctx.cwd.lock().unwrap().clone();
        assert!(new_cwd.to_string_lossy().contains("var"));
    }
}
