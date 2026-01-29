use super::{Tool, ExecutionContext};
use tokio::process::Command;

pub struct DiffTool;

#[async_trait::async_trait]
impl Tool for DiffTool {
    fn name(&self) -> &str {
        "diff"
    }

    fn description(&self) -> &str {
        "Compare files or show git diff (e.g. !diff file1 file2 or !diff)"
    }

    async fn execute(&self, args: &str, _ctx: &ExecutionContext) -> String {
        let args = args.trim();

        let output = if args.is_empty() {
            Command::new("git")
                .arg("diff")
                .arg("--stat")
                .output()
                .await
        } else {
            let parts: Vec<&str> = args.split_whitespace().collect();
            if parts.len() == 1 {
                Command::new("git")
                    .arg("diff")
                    .arg(parts[0])
                    .output()
                    .await
            } else if parts.len() == 2 {
                Command::new("diff")
                    .arg("-u")
                    .arg(parts[0])
                    .arg(parts[1])
                    .output()
                    .await
            } else {
                return "usage: !diff [file] | !diff <file1> <file2>".to_string();
            }
        };

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if !stderr.is_empty() && stdout.is_empty() {
                    return format!("error: {}", stderr.trim());
                }

                if stdout.is_empty() {
                    "no differences".to_string()
                } else {
                    truncate_lines(&stdout, 30)
                }
            }
            Err(e) => format!("error: {}", e),
        }
    }
}

fn truncate_lines(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().take(max_lines).collect();
    let total = s.lines().count();
    let mut result = lines.join("\n");
    if total > max_lines {
        result.push_str(&format!("\n... ({} more lines)", total - max_lines));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_utils::test_context;

    fn test_ctx() -> ExecutionContext {
        test_context("/tmp")
    }

    #[tokio::test]
    async fn test_diff_git() {
        let tool = DiffTool;
        let result = tool.execute("", &test_ctx()).await;
        assert!(!result.contains("usage"));
    }
}
