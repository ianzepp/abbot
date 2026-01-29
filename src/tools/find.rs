use super::{Tool, ExecutionContext};
use tokio::process::Command;

pub struct FindTool;

#[async_trait::async_trait]
impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "Find files by name pattern (e.g. !find *.rs, !find path=src *.rs, !find type=d path=src * )"
    }

    async fn execute(&self, args: &str, _ctx: &ExecutionContext) -> String {
        let args = args.trim();
        if args.is_empty() {
            return "usage: !find [path=<path>] [type=f|d] <pattern>".to_string();
        }

        let mut pattern: Option<&str> = None;
        let mut path: &str = ".";
        let mut type_flag: &str = "f";

        for part in args.split_whitespace() {
            if let Some(rest) = part.strip_prefix("path=") {
                if !rest.is_empty() {
                    path = rest;
                }
                continue;
            }

            if let Some(rest) = part.strip_prefix("type=") {
                if rest == "f" || rest == "d" {
                    type_flag = rest;
                    continue;
                }
                return "usage: !find [path=<path>] [type=f|d] <pattern>".to_string();
            }

            if pattern.is_none() {
                pattern = Some(part);
                continue;
            }

            // Back-compat: allow "<pattern> <path>" (two bare tokens).
            if path == "." {
                path = part;
                continue;
            }

            return "usage: !find [path=<path>] [type=f|d] <pattern>".to_string();
        }

        let Some(pattern) = pattern else {
            return "usage: !find [path=<path>] [type=f|d] <pattern>".to_string();
        };

        match Command::new("find")
            .arg(path)
            .arg("-name")
            .arg(pattern)
            .arg("-type")
            .arg(type_flag)
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if !stderr.is_empty() && stdout.is_empty() {
                    return format!("error: {}", stderr.trim());
                }

                let lines: Vec<&str> = stdout.lines().take(1000).collect();
                if lines.is_empty() {
                    "no files found".to_string()
                } else {
                    let mut result = lines.join("\n");
                    let total = stdout.lines().count();
                    if total > 1000 {
                        result.push_str(&format!("\n... and {} more", total - 1000));
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

    #[tokio::test]
    async fn test_find_path_equals() {
        let tool = FindTool;
        let result = tool.execute("path=. Cargo.toml", &test_ctx()).await;
        assert!(result.contains("Cargo.toml"));
    }

    #[tokio::test]
    async fn test_find_type_dir() {
        let tool = FindTool;
        let result = tool.execute("type=d path=. src", &test_ctx()).await;
        assert!(result.contains("src") || result.contains("no files found") || result.contains("error:"));
    }
}
