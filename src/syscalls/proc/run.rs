use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::hal::{HalProcess, HalProcessError, HostHalProcess};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

const DEFAULT_ALLOWED_PROGRAMS: &[&str] = &[
    "git", "cargo", "npm", "npx", "node", "python", "python3", "ls", "find", "cat", "head", "tail",
    "grep", "rg", "sed", "awk", "sort", "uniq", "wc", "diff", "patch", "tar", "gzip", "gunzip",
    "zip", "unzip", "curl", "wget", "jq", "yq", "make", "cmake", "rustc", "rustfmt", "clippy",
    "tsc", "eslint", "prettier", "go", "gofmt", "ruby", "perl", "php", "java", "javac", "mvn",
    "gradle", "pytest", "jest", "mocha", "rspec", "echo", "printf", "true", "false", "test",
    "mkdir", "rmdir", "rm", "cp", "mv", "touch", "chmod", "date", "env", "which", "whoami",
    "sleep",
];

#[derive(Debug, Deserialize)]
struct ProcRunArgs {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    stdin: Option<String>,
    #[serde(default)]
    max_stdout: Option<usize>,
    #[serde(default)]
    max_stderr: Option<usize>,
}

pub struct ProcRun {
    proc: Arc<dyn HalProcess>,
    allowed: Vec<String>,
}

impl ProcRun {
    pub fn new() -> Self {
        Self {
            proc: Arc::new(HostHalProcess),
            allowed: DEFAULT_ALLOWED_PROGRAMS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    pub fn with_process(proc: Arc<dyn HalProcess>) -> Self {
        Self {
            proc,
            allowed: DEFAULT_ALLOWED_PROGRAMS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    pub fn with_allowed(mut self, programs: Vec<String>) -> Self {
        self.allowed = programs;
        self
    }

    pub fn add_allowed(&mut self, program: impl Into<String>) {
        self.allowed.push(program.into());
    }

    fn is_allowed(&self, program: &str) -> bool {
        let basename = std::path::Path::new(program)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(program);
        self.allowed.iter().any(|a| a == basename)
    }
}

impl Default for ProcRun {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for ProcRun {
    fn name(&self) -> &'static str {
        "proc:run"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;

        let args: ProcRunArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.program.is_empty() {
            return Err(KernelError::invalid_args("'program' is required"));
        }

        if !self.is_allowed(&args.program) {
            return Err(KernelError::forbidden(format!(
                "program '{}' is not in the allowed list",
                args.program
            ))
            .with_help(format!("Allowed programs: {}", self.allowed.join(", "))));
        }

        let cwd = if let Some(ref cwd_str) = args.cwd {
            let vfs = MountTable::global().ok_or_else(|| {
                KernelError::disabled("filesystem access disabled: no mounts configured")
            })?;
            let resolved = vfs.resolve(cwd_str)?;
            resolved.host_path
        } else {
            ctx.cwd.clone()
        };

        let timeout = args
            .timeout_ms
            .or(ctx.deadline_ms)
            .map(Duration::from_millis);

        let max_stdout = args.max_stdout.unwrap_or(2 * 1024 * 1024);
        let max_stderr = args.max_stderr.unwrap_or(512 * 1024);

        ctx.check_cancelled()?;

        let child_cancel = CancellationToken::new();
        let parent_cancel = ctx.cancel.clone();
        let child_cancel_clone = child_cancel.clone();

        tokio::spawn(async move {
            parent_cancel.cancelled().await;
            child_cancel_clone.cancel();
        });

        let result = if let Some(stdin) = args.stdin {
            self.proc
                .run_with_stdin_bytes_bounded(
                    &args.program,
                    &args.args,
                    &cwd,
                    args.env.as_ref(),
                    timeout,
                    stdin.as_bytes(),
                    max_stdout,
                    max_stderr,
                    Some(child_cancel),
                )
                .await
        } else {
            self.proc
                .run_bounded(
                    &args.program,
                    &args.args,
                    &cwd,
                    args.env.as_ref(),
                    timeout,
                    max_stdout,
                    max_stderr,
                    Some(child_cancel),
                )
                .await
        };

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
            Err(HalProcessError::Cancelled { program }) => {
                Err(KernelError::cancelled(format!("{program} was cancelled")))
            }
            Err(HalProcessError::Timeout { program, timeout }) => Err(KernelError::timeout(
                format!("{program} timed out after {:?}", timeout),
            )),
            Err(HalProcessError::Io(msg)) => Err(KernelError::io(msg)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn make_ctx_with_actor(cwd: &std::path::Path, actor: &str) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
            .with_actor(Some(actor.to_string()))
    }

    fn make_ctx(cwd: &std::path::Path) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
    }

    #[tokio::test]
    async fn test_proc_run_requires_head_scope() {
        let tmp = TempDir::new().unwrap();
        let syscall = ProcRun::new();
        let ctx = make_ctx(tmp.path());
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "program": "echo", "args": ["hello"] }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_proc_run_hand_scope_rejected() {
        let tmp = TempDir::new().unwrap();
        let syscall = ProcRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "hand/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "program": "echo", "args": ["hello"] }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_proc_run_echo_with_head_scope() {
        let tmp = TempDir::new().unwrap();
        let syscall = ProcRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "head/test");
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "program": "echo", "args": ["hello", "world"] }),
                tx,
            )
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.op, crate::kernel::FrameOp::Ok);

        let data = frame.data.unwrap();
        assert!(data["success"].as_bool().unwrap());
        assert!(data["stdout"].as_str().unwrap().contains("hello world"));
    }

    #[tokio::test]
    async fn test_proc_run_forbidden_program() {
        let tmp = TempDir::new().unwrap();
        let syscall = ProcRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "head/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall.execute(&ctx, json!({ "program": "nc" }), tx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_proc_run_git_status_with_head_scope() {
        let tmp = TempDir::new().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .ok();

        let syscall = ProcRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "head/test");
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "program": "git", "args": ["status"] }), tx)
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.op, crate::kernel::FrameOp::Ok);
        assert!(frame.data.unwrap()["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_proc_run_with_stdin() {
        let tmp = TempDir::new().unwrap();
        let syscall = ProcRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "head/test");
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "program": "cat", "stdin": "piped input" }),
                tx,
            )
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        let data = frame.data.unwrap();
        assert_eq!(data["stdout"].as_str().unwrap(), "piped input");
    }

    #[test]
    fn test_allowed_program_check() {
        let syscall = ProcRun::new();
        assert!(syscall.is_allowed("git"));
        assert!(syscall.is_allowed("cargo"));
        assert!(syscall.is_allowed("/usr/bin/git"));
        assert!(!syscall.is_allowed("nc"));
        assert!(!syscall.is_allowed("netcat"));
        assert!(!syscall.is_allowed("/bin/sh"));
    }
}
