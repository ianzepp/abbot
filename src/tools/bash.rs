use super::Tool;
use tokio::process::Command;

pub struct BashTool;

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command"
    }

    async fn execute(&self, args: &str) -> String {
        if args.trim().is_empty() {
            return "usage: !bash <command>".to_string();
        }

        match Command::new("bash")
            .arg("-c")
            .arg(args)
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&stderr);
                }

                if result.is_empty() {
                    "(no output)".to_string()
                } else {
                    truncate_output(&result, 400)
                }
            }
            Err(e) => format!("error: {}", e),
        }
    }
}

fn truncate_output(s: &str, max_chars: usize) -> String {
    let s = s.trim();
    if s.len() <= max_chars {
        s.to_string()
    } else {
        format!("{}... (truncated)", &s[..max_chars])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bash_echo() {
        let tool = BashTool;
        let result = tool.execute("echo hello").await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_bash_empty() {
        let tool = BashTool;
        let result = tool.execute("").await;
        assert!(result.contains("usage"));
    }
}
