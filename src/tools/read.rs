use super::{Tool, ExecutionContext};
use std::path::Path;
use tokio::fs;

const MAX_LINES: usize = 1500;

pub struct ReadTool;

#[async_trait::async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read file contents"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let (path, offset, limit) = match parse_args(args) {
            Ok(parsed) => parsed,
            Err(e) => return e,
        };

        // Resolve path relative to cwd
        let full_path = if Path::new(&path).is_absolute() {
            path.clone()
        } else {
            let cwd = ctx.cwd.lock().unwrap();
            cwd.join(&path).to_string_lossy().to_string()
        };

        let content = match fs::read_to_string(&full_path).await {
            Ok(c) => c,
            Err(e) => return format!("error: {}", e),
        };

        let all_lines: Vec<&str> = content.lines().collect();
        let total = all_lines.len();

        // If no offset/limit and file is too large, return error
        if offset.is_none() && limit.is_none() && total > MAX_LINES {
            return format!(
                "error: file has {} lines (max {}). Use offset= and limit= to read sections.",
                total, MAX_LINES
            );
        }

        let start = offset.unwrap_or(0);
        let count = limit.unwrap_or(MAX_LINES).min(MAX_LINES);

        if start >= total {
            return format!("error: offset {} exceeds file length ({})", start, total);
        }

        let end = (start + count).min(total);
        let lines = &all_lines[start..end];

        let mut result = String::new();
        for line in lines.iter() {
            result.push_str(line);
            result.push('\n');
        }

        // Add summary if truncated
        if end < total {
            result.push_str(&format!("... ({} more lines)\n", total - end));
        }

        result
    }
}

fn parse_args(args: &str) -> Result<(String, Option<usize>, Option<usize>), String> {
    let args = args.trim();
    if args.is_empty() {
        return Err("usage: read <path> [offset=N] [limit=N]".to_string());
    }

    let mut path = String::new();
    let mut offset = None;
    let mut limit = None;

    for part in args.split_whitespace() {
        if let Some(val) = part.strip_prefix("offset=") {
            offset = Some(val.parse::<usize>().map_err(|_| "error: invalid offset")?);
        } else if let Some(val) = part.strip_prefix("limit=") {
            limit = Some(val.parse::<usize>().map_err(|_| "error: invalid limit")?);
        } else if path.is_empty() {
            path = part.to_string();
        } else {
            return Err(format!("error: unexpected argument '{}'", part));
        }
    }

    if path.is_empty() {
        return Err("usage: read <path> [offset=N] [limit=N]".to_string());
    }

    Ok((path, offset, limit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_utils::test_context;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_read_empty() {
        let tool = ReadTool;
        let ctx = test_context("/tmp");
        let result = tool.execute("", &ctx).await;
        assert!(result.contains("usage"));
    }

    #[tokio::test]
    async fn test_read_cargo() {
        let tool = ReadTool;
        // Use project root as cwd so relative path works
        let ctx = test_context(std::env::current_dir().unwrap());
        let result = tool.execute("Cargo.toml", &ctx).await;
        assert!(result.contains("[package]"), "got: {}", result);
    }

    #[tokio::test]
    async fn test_read_nonexistent() {
        let tool = ReadTool;
        let ctx = test_context("/tmp");
        let result = tool.execute("/nonexistent/file", &ctx).await;
        assert!(result.contains("error"));
    }

    #[tokio::test]
    async fn test_read_content() {
        let tool = ReadTool;
        let ctx = test_context(std::env::current_dir().unwrap());
        let result = tool.execute("Cargo.toml", &ctx).await;
        // Should have content without line numbers
        assert!(result.contains("[package]"), "should have package section");
        assert!(!result.contains("    1  "), "should not have line numbers");
    }

    #[tokio::test]
    async fn test_read_with_offset_limit() {
        let tool = ReadTool;
        let ctx = test_context(std::env::current_dir().unwrap());
        let result = tool.execute("Cargo.toml offset=2 limit=3", &ctx).await;
        // Should have 3 lines of content
        let lines: Vec<&str> = result.lines().collect();
        assert!(lines.len() >= 3, "should have at least 3 lines, got: {}", result);
    }

    #[tokio::test]
    async fn test_read_too_large() {
        use tokio::fs;

        let dir = PathBuf::from("/tmp/read_test_large");
        let _ = fs::create_dir_all(&dir).await;
        let file = dir.join("big.txt");

        // Create a file with more than MAX_LINES
        let content: String = (0..2000).map(|i| format!("line {}\n", i)).collect();
        fs::write(&file, &content).await.unwrap();

        let tool = ReadTool;
        let ctx = test_context(&dir);
        let result = tool.execute("big.txt", &ctx).await;

        assert!(result.contains("error"), "should error on large file: {}", result);
        assert!(result.contains("2000"), "should mention line count");
        assert!(result.contains("offset="), "should suggest offset/limit");

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_parse_args() {
        let (path, offset, limit) = parse_args("foo.txt").unwrap();
        assert_eq!(path, "foo.txt");
        assert_eq!(offset, None);
        assert_eq!(limit, None);

        let (path, offset, limit) = parse_args("foo.txt offset=10").unwrap();
        assert_eq!(path, "foo.txt");
        assert_eq!(offset, Some(10));
        assert_eq!(limit, None);

        let (path, offset, limit) = parse_args("foo.txt offset=10 limit=50").unwrap();
        assert_eq!(path, "foo.txt");
        assert_eq!(offset, Some(10));
        assert_eq!(limit, Some(50));

        let (path, offset, limit) = parse_args("foo.txt limit=50").unwrap();
        assert_eq!(path, "foo.txt");
        assert_eq!(offset, None);
        assert_eq!(limit, Some(50));
    }
}
