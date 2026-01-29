use super::{Tool, ExecutionContext};
use std::process::Stdio;
use tokio::process::Command;
use tokio::io::AsyncWriteExt;

pub struct PatchTool;

#[async_trait::async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch"
    }

    async fn execute(&self, args: &str, ctx: &ExecutionContext) -> String {
        let diff = args.trim();
        if diff.is_empty() {
            return "usage: patch <unified-diff>".to_string();
        }

        if !diff.contains("---") || !diff.contains("+++") {
            return "error: invalid diff format (missing --- or +++ headers)".to_string();
        }

        let mut child = match Command::new("patch")
            .arg("-p1")
            .arg("--no-backup-if-mismatch")
            .arg("-r-")
            .current_dir(&ctx.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return format!("error spawning patch: {}", e),
        };

        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(diff.as_bytes()).await {
                return format!("error writing to patch stdin: {}", e);
            }
        }

        match child.wait_with_output().await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if output.status.success() {
                    if stdout.trim().is_empty() {
                        "patched".to_string()
                    } else {
                        stdout.trim().to_string()
                    }
                } else {
                    let msg = if !stderr.is_empty() {
                        stderr.trim()
                    } else if !stdout.is_empty() {
                        stdout.trim()
                    } else {
                        "patch failed"
                    };
                    format!("error: {}", msg)
                }
            }
            Err(e) => format!("error: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::fs;
    use crate::tools::test_utils::test_context;

    fn test_ctx() -> ExecutionContext {
        test_context("/tmp")
    }

    #[tokio::test]
    async fn test_patch_empty() {
        let tool = PatchTool;
        let result = tool.execute("", &test_ctx()).await;
        assert!(result.contains("usage"));
    }

    #[tokio::test]
    async fn test_patch_invalid_format() {
        let tool = PatchTool;
        let result = tool.execute("not a diff", &test_ctx()).await;
        assert!(result.contains("error"));
    }

    #[tokio::test]
    async fn test_patch_apply() {
        let dir = PathBuf::from("/tmp/patch_test");
        let _ = fs::create_dir_all(&dir).await;
        let file = dir.join("test.txt");
        fs::write(&file, "line1\nline2\nline3\n").await.unwrap();

        let ctx = test_context(&dir);

        let diff = r#"--- a/test.txt
+++ b/test.txt
@@ -1,3 +1,3 @@
 line1
-line2
+line2 modified
 line3
"#;

        let tool = PatchTool;
        let result = tool.execute(diff, &ctx).await;
        assert!(!result.contains("error"), "got: {}", result);

        let content = fs::read_to_string(&file).await.unwrap();
        assert!(content.contains("line2 modified"));

        let _ = fs::remove_dir_all(&dir).await;
    }
}
