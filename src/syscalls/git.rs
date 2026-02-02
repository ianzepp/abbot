use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::hal::{HalProcess, HalProcessError, HostHalProcess};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

const FORBIDDEN_GIT_COMMANDS: &[&str] = &["push", "credential", "config", "remote"];

const MUTATING_GIT_COMMANDS: &[&str] = &[
    "add", "commit", "merge", "rebase", "reset", "checkout", "switch", "restore", "stash", "cherry-pick",
    "revert", "clean", "rm", "mv", "branch", "tag", "fetch", "pull", "clone", "init", "worktree",
    "submodule", "apply", "am",
];

const DANGEROUS_FLAGS: &[&str] = &[
    "--exec",
    "-c",
];

#[derive(Debug, Deserialize)]
struct GitRunArgs {
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub struct GitRun {
    proc: Arc<dyn HalProcess>,
}

impl GitRun {
    pub fn new() -> Self {
        Self {
            proc: Arc::new(HostHalProcess),
        }
    }

    pub fn with_process(proc: Arc<dyn HalProcess>) -> Self {
        Self { proc }
    }

    fn validate_args(&self, args: &[String]) -> Result<(), KernelError> {
        if args.is_empty() {
            return Ok(());
        }

        let subcommand = &args[0];
        if FORBIDDEN_GIT_COMMANDS.contains(&subcommand.as_str()) {
            return Err(KernelError::forbidden(format!(
                "git subcommand '{}' is not allowed",
                subcommand
            ))
            .with_help("Use read-only git commands like status, log, diff, show, branch, etc."));
        }

        for arg in args {
            for flag in DANGEROUS_FLAGS {
                if arg.starts_with(flag) {
                    return Err(KernelError::forbidden(format!(
                        "git flag '{}' is not allowed",
                        arg
                    )));
                }
            }
        }

        Ok(())
    }

    fn is_mutating(&self, args: &[String]) -> bool {
        if args.is_empty() {
            return false;
        }
        MUTATING_GIT_COMMANDS.contains(&args[0].as_str())
    }
}

impl Default for GitRun {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for GitRun {
    fn name(&self) -> &'static str {
        "git:run"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: GitRunArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        self.validate_args(&args.args)?;

        if self.is_mutating(&args.args) {
            ctx.require_mutation()?;
        }

        let cwd = if let Some(ref cwd_str) = args.cwd {
            let vfs = MountTable::global()
                .ok_or_else(|| KernelError::disabled("filesystem access disabled: no mounts configured"))?;
            let resolved = vfs.resolve(cwd_str)?;
            resolved.host_path
        } else {
            ctx.cwd.clone()
        };

        let timeout = args
            .timeout_ms
            .or(ctx.deadline_ms)
            .map(Duration::from_millis);

        const MAX_STDOUT: usize = 2 * 1024 * 1024;
        const MAX_STDERR: usize = 512 * 1024;

        ctx.check_cancelled()?;

        let child_cancel = CancellationToken::new();
        let parent_cancel = ctx.cancel.clone();
        let child_cancel_clone = child_cancel.clone();

        tokio::spawn(async move {
            parent_cancel.cancelled().await;
            child_cancel_clone.cancel();
        });

        let result = self
            .proc
            .run_bounded(
                "git",
                &args.args,
                &cwd,
                None,
                timeout,
                MAX_STDOUT,
                MAX_STDERR,
                Some(child_cancel),
            )
            .await;

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                tx.send(Frame::ok(
                    ctx.call_id,
                    json!({
                        "code": output.code,
                        "success": output.success,
                        "stdout": stdout,
                        "stderr": stderr,
                        "stdout_truncated": output.stdout_truncated,
                        "stderr_truncated": output.stderr_truncated,
                    }),
                ))
                .await
                .ok();
                Ok(())
            }
            Err(HalProcessError::Cancelled { .. }) => {
                Err(KernelError::cancelled("git command was cancelled"))
            }
            Err(HalProcessError::Timeout { timeout, .. }) => Err(KernelError::timeout(format!(
                "git command timed out after {:?}",
                timeout
            ))),
            Err(HalProcessError::Io(msg)) => Err(KernelError::io(msg)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn make_ctx(cwd: &std::path::Path) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
    }

    fn make_ctx_with_scope(cwd: &std::path::Path, scope: &str) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
            .with_scope(Some(scope.to_string()))
    }

    #[tokio::test]
    async fn test_git_status_readonly_allowed() {
        let tmp = TempDir::new().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .ok();

        let syscall = GitRun::new();
        let ctx = make_ctx(tmp.path());
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "args": ["status"] }), tx)
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.op, crate::kernel::FrameOp::Ok);
        assert!(frame.data.unwrap()["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_git_log_readonly_allowed() {
        let tmp = TempDir::new().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .ok();

        let syscall = GitRun::new();
        let ctx = make_ctx(tmp.path());
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "args": ["log", "--oneline", "-n", "5"] }), tx)
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.op, crate::kernel::FrameOp::Ok);
    }

    #[tokio::test]
    async fn test_git_add_requires_mutation() {
        let tmp = TempDir::new().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .ok();

        let syscall = GitRun::new();
        let ctx = make_ctx(tmp.path());
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "args": ["add", "."] }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_git_add_with_head_scope() {
        let tmp = TempDir::new().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .ok();

        let syscall = GitRun::new();
        let ctx = make_ctx_with_scope(tmp.path(), "head/test");
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "args": ["add", "."] }), tx)
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.op, crate::kernel::FrameOp::Ok);
    }

    #[tokio::test]
    async fn test_git_push_forbidden() {
        let tmp = TempDir::new().unwrap();
        let syscall = GitRun::new();
        let ctx = make_ctx_with_scope(tmp.path(), "head/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "args": ["push", "origin", "main"] }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_git_config_forbidden() {
        let tmp = TempDir::new().unwrap();
        let syscall = GitRun::new();
        let ctx = make_ctx_with_scope(tmp.path(), "head/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "args": ["config", "user.email", "test@example.com"] }),
                tx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[test]
    fn test_validate_args() {
        let syscall = GitRun::new();

        assert!(syscall.validate_args(&["status".to_string()]).is_ok());
        assert!(syscall.validate_args(&["log".to_string()]).is_ok());
        assert!(syscall.validate_args(&["diff".to_string()]).is_ok());
        assert!(syscall.validate_args(&["push".to_string()]).is_err());
        assert!(syscall.validate_args(&["config".to_string()]).is_err());
        assert!(syscall
            .validate_args(&["log".to_string(), "--exec=malicious".to_string()])
            .is_err());
    }

    #[test]
    fn test_is_mutating() {
        let syscall = GitRun::new();

        assert!(!syscall.is_mutating(&["status".to_string()]));
        assert!(!syscall.is_mutating(&["log".to_string()]));
        assert!(!syscall.is_mutating(&["diff".to_string()]));
        assert!(!syscall.is_mutating(&["show".to_string()]));
        assert!(!syscall.is_mutating(&[]));

        assert!(syscall.is_mutating(&["add".to_string()]));
        assert!(syscall.is_mutating(&["commit".to_string()]));
        assert!(syscall.is_mutating(&["merge".to_string()]));
        assert!(syscall.is_mutating(&["rebase".to_string()]));
        assert!(syscall.is_mutating(&["checkout".to_string()]));
    }
}
