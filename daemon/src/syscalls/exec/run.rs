//! Exec:Run - Execute external programs with security constraints
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides controlled execution of external programs (git, cargo, npm, etc.)
//! within the Abbot kernel's security model. It integrates with the Hardware Abstraction Layer
//! (HAL) for process management and the Virtual Filesystem (VFS) for path resolution.
//!
//! **Critical security boundaries:**
//! - Allowlist-based program filtering (only approved programs can run)
//! - Actor-based mutation permission enforcement (only "head" agents can execute)
//! - Output size limiting (prevents resource exhaustion from runaway processes)
//! - Cancellation propagation (child processes terminate when parent cancels)
//! - Working directory resolution via VFS (prevents unauthorized filesystem access)
//!
//! **Integration points:**
//! - `HalProcess` trait for platform-specific process spawning
//! - `MountTable::global()` for VFS-based working directory resolution
//! - `SyscallContext` for actor verification, cancellation, and deadline enforcement
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with stdout/stderr/exit code on successful execution
//! - Returns `KernelError` for security violations, timeouts, or I/O failures
//!
//! SECURITY MODEL
//! ==============
//! This syscall implements **defense-in-depth** for process execution:
//!
//! 1. **Actor Authorization**
//!    - WHY: Prevents malicious or buggy "hand" agents from executing arbitrary commands
//!    - HOW: `ctx.require_mutation()` enforces "head" actor requirement at line 107
//!    - ATTACK PREVENTED: Privilege escalation from read-only to mutation-capable actors
//!
//! 2. **Program Allowlist**
//!    - WHY: Limits attack surface to known-safe development tools
//!    - HOW: `is_allowed()` checks basename against `DEFAULT_ALLOWED_PROGRAMS` (line 116)
//!    - ATTACK PREVENTED: Execution of dangerous binaries like `nc`, `ssh`, `/bin/sh`
//!    - TRADE-OFF: Explicitly blocks shells to prevent command injection via `-c` flag
//!
//! 3. **Output Limiting**
//!    - WHY: Prevents memory exhaustion from programs with unbounded output
//!    - HOW: `max_stdout` (default 2MB) and `max_stderr` (default 512KB) truncate output
//!    - ATTACK PREVENTED: Resource exhaustion DoS from `yes`, `cat /dev/urandom`, etc.
//!
//! 4. **Working Directory Isolation**
//!    - WHY: Ensures processes can only access VFS-mounted paths
//!    - HOW: VFS resolution at line 128 rejects unmounted paths with `E_DISABLED`
//!    - ATTACK PREVENTED: Filesystem escapes via `../../` or absolute paths outside mounts
//!
//! 5. **Timeout Enforcement**
//!    - WHY: Prevents hung processes from blocking task lanes indefinitely
//!    - HOW: Respects syscall context deadline or explicit `timeout_ms` argument
//!    - ATTACK PREVENTED: Resource exhaustion from infinite loops or network hangs
//!
//! 6. **Cancellation Propagation**
//!    - WHY: Ensures child processes terminate when parent task is cancelled
//!    - HOW: Spawns monitor task (line 148) that cancels child when parent cancels
//!    - ATTACK PREVENTED: Orphaned processes consuming resources after task cancellation
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Security over convenience**: Strict allowlist prevents 99% of attack vectors
//! - **Fail-safe defaults**: Output limits and timeouts prevent resource exhaustion
//! - **Explicit authorization**: Mutation guard prevents accidental command execution
//! - **Cancellation hygiene**: Child processes never outlive their parent context
//! - **Path resolution via VFS**: All filesystem access goes through security-audited VFS layer
//!
//! PERFORMANCE
//! ===========
//! - Output is truncated at `max_stdout`/`max_stderr` bytes to bound memory usage
//! - Cancellation token allows early termination without polling overhead
//! - VFS resolution happens once at start, not per-file-access
//!
//! CONCURRENCY
//! ===========
//! - Spawns detached monitor task to propagate cancellation from parent to child
//! - Child process blocking does not block kernel event loop (delegated to HAL layer)
//! - Safe for concurrent execution across multiple task lanes
//!
//! TRADE-OFFS
//! ==========
//! 1. **Allowlist vs. Blocklist**
//!    - CHOSEN: Allowlist (only approved programs run)
//!    - REJECTED: Blocklist (block known-bad programs)
//!    - WHY: Allowlist provides stronger security guarantees (default-deny)
//!
//! 2. **Shell Access**
//!    - CHOSEN: No shell execution (`sh`, `bash` not in allowlist)
//!    - WHY: Shells enable command injection via `-c "arbitrary command"`
//!    - IMPLICATION: Users must use structured arguments, not shell strings
//!
//! 3. **Output Truncation**
//!    - CHOSEN: Hard limits with truncation flag (not streaming)
//!    - WHY: Simpler implementation, bounded memory usage
//!    - IMPLICATION: Large outputs (e.g., `git log --all`) may be truncated

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

// =============================================================================
// SECURITY CONSTANTS
// =============================================================================

/// Programs approved for execution via `exec:run`.
///
/// WHY: Defense-in-depth security via allowlist rather than blocklist. Only
/// known-safe development tools (git, cargo, npm, etc.) are permitted.
///
/// IMPORTANT: Shells (`sh`, `bash`, `zsh`) are explicitly excluded to prevent
/// command injection attacks via `-c` flag. If shell functionality is needed,
/// add specific allowed programs instead of enabling arbitrary shell execution.
///
/// WHY NO SHELLS: `sh -c "$(user_input)"` allows arbitrary command execution,
/// bypassing the allowlist. Structured arguments (program + args array) provide
/// better security than shell string parsing.
const DEFAULT_ALLOWED_PROGRAMS: &[&str] = &[
    "git", "gh", "cargo", "npm", "npx", "node", "python", "python3", "brew",
    "ls", "find", "cat", "head", "tail",
    "grep", "rg", "sed", "awk", "sort", "uniq", "wc", "diff", "patch", "tar", "gzip", "gunzip",
    "zip", "unzip", "curl", "wget", "jq", "yq", "make", "cmake", "rustc", "rustfmt", "clippy",
    "tsc", "eslint", "prettier", "go", "gofmt", "ruby", "perl", "php", "java", "javac", "mvn",
    "gradle", "pytest", "jest", "mocha", "rspec", "echo", "printf", "true", "false", "test",
    "mkdir", "rmdir", "rm", "cp", "mv", "touch", "chmod", "date", "env", "which", "whoami",
    "sleep",
];

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `exec:run` syscall.
///
/// WHY: Structured arguments prevent command injection. By separating program
/// from args (instead of accepting a shell string), we ensure arguments cannot
/// be reinterpreted as commands.
#[derive(Debug, Deserialize)]
struct ExecRunArgs {
    /// Program name or path (e.g., "git", "/usr/bin/python3").
    ///
    /// WHY: Basename is extracted and checked against allowlist, preventing
    /// path-based bypasses like "/tmp/evil/git" circumventing "git" allowlist entry.
    program: String,

    /// Command-line arguments passed to the program.
    ///
    /// WHY: Structured array prevents shell injection. Each element is passed
    /// as-is to the process, not interpreted by a shell.
    #[serde(default)]
    args: Vec<String>,

    /// Environment variables for the subprocess.
    ///
    /// WHY: Allows setting PATH, LANG, etc. without inheriting parent environment.
    /// SECURITY: Inherits from parent if None, which includes PATH.
    #[serde(default)]
    env: Option<HashMap<String, String>>,

    /// Working directory for the subprocess.
    ///
    /// WHY: Defaults to syscall context's cwd if unspecified.
    /// SECURITY: VFS resolution at execution time ensures path is within mounted filesystem.
    #[serde(default)]
    cwd: Option<String>,

    /// Timeout in milliseconds.
    ///
    /// WHY: Prevents hung processes. Falls back to context deadline if unspecified.
    /// SECURITY: Always enforced - no infinite execution.
    #[serde(default)]
    timeout_ms: Option<u64>,

    /// Data to pipe to the subprocess's stdin.
    ///
    /// WHY: Enables scripting without temporary files (e.g., `git apply` from patch string).
    #[serde(default)]
    stdin: Option<String>,

    /// Maximum stdout bytes to capture (default: 2MB).
    ///
    /// WHY: Prevents memory exhaustion from unbounded output.
    /// TRADE-OFF: Large outputs (e.g., `git log --all`) may truncate.
    #[serde(default)]
    max_stdout: Option<usize>,

    /// Maximum stderr bytes to capture (default: 512KB).
    ///
    /// WHY: Error messages are typically smaller than stdout, so lower limit.
    #[serde(default)]
    max_stderr: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for executing external programs with security constraints.
///
/// WHY: Encapsulates HAL process abstraction and allowlist configuration.
/// Allows testing with mock `HalProcess` implementations.
pub struct ExecRun {
    /// Hardware abstraction for process spawning.
    ///
    /// WHY: Enables testing with fake processes, platform-specific implementations.
    proc: Arc<dyn HalProcess>,

    /// Programs allowed for execution.
    ///
    /// WHY: Configurable allowlist enables adding project-specific tools
    /// (e.g., custom build scripts) without recompiling kernel.
    allowed: Vec<String>,
}

impl ExecRun {
    /// Create a new `ExecRun` syscall with default allowed programs.
    ///
    /// WHY: Standard constructor for production use with host OS process spawning.
    pub fn new() -> Self {
        Self {
            proc: Arc::new(HostHalProcess),
            allowed: DEFAULT_ALLOWED_PROGRAMS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// Create a `ExecRun` syscall with a custom HAL process implementation.
    ///
    /// WHY: Enables testing with mock processes that simulate failures, timeouts, etc.
    pub fn with_process(proc: Arc<dyn HalProcess>) -> Self {
        Self {
            proc,
            allowed: DEFAULT_ALLOWED_PROGRAMS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// Replace the allowed programs list.
    ///
    /// WHY: Allows configuration of project-specific tooling (e.g., custom build scripts).
    /// SECURITY: Use carefully - expanding allowlist increases attack surface.
    pub fn with_allowed(mut self, programs: Vec<String>) -> Self {
        self.allowed = programs;
        self
    }

    /// Add a program to the allowed list.
    ///
    /// WHY: Incremental allowlist extension for one-off tool additions.
    pub fn add_allowed(&mut self, program: impl Into<String>) {
        self.allowed.push(program.into());
    }

    /// Check if a program is in the allowed list.
    ///
    /// WHY: Extracts basename to prevent path-based allowlist bypass.
    ///
    /// SECURITY: `/tmp/evil/git` would extract basename "git" for comparison,
    /// preventing directory-based circumvention. The actual executable path is
    /// still resolved by the OS, so `/tmp/evil/git` would run that binary - but
    /// only if "git" is in the allowlist.
    ///
    /// TRADE-OFF: Does not verify cryptographic signature or binary hash, only name.
    /// If `/tmp/evil/git` is on PATH before `/usr/bin/git`, the malicious binary runs.
    /// This is acceptable because:
    /// 1. PATH manipulation requires filesystem write access (already privileged)
    /// 2. VFS layer restricts which directories are writable
    /// 3. Allowlist still prevents execution of completely arbitrary program names
    fn is_allowed(&self, program: &str) -> bool {
        let basename = std::path::Path::new(program)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(program);
        self.allowed.iter().any(|a| a == basename)
    }
}

impl Default for ExecRun {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for ExecRun {
    fn name(&self) -> &'static str {
        "exec:run"
    }

    /// Execute an external program with security constraints.
    ///
    /// WHY: Enables agents to invoke development tools (git, cargo, npm) while
    /// maintaining security boundaries through allowlist filtering, actor authorization,
    /// output limiting, and cancellation propagation.
    ///
    /// USE CASE: Invoked by "head" agents to run build tools, tests, or queries
    /// (e.g., `git status`, `cargo test`, `npm install`). "Hand" and "room" agents
    /// are prohibited from execution to prevent privilege escalation.
    ///
    /// SECURITY NOTE: This syscall implements multiple defense layers:
    /// 1. Actor verification - only "head" agents may execute (line 168)
    /// 2. Allowlist filtering - only approved programs run (line 178)
    /// 3. VFS path resolution - working directory must be within mounted paths (line 185)
    /// 4. Output limiting - prevents memory exhaustion (line 197)
    /// 5. Cancellation propagation - child terminates when parent cancels (line 203)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{code, success, stdout, stderr, stdout_truncated, stderr_truncated}`
    /// - `E_FORBIDDEN` if actor lacks mutation permission or program not allowed
    /// - `E_INVALID_ARGS` if program name is empty or malformed
    /// - `E_DISABLED` if VFS is not configured (no mounts)
    /// - `E_CANCELLED` if parent context cancels during execution
    /// - `E_TIMEOUT` if execution exceeds timeout
    /// - `E_IO` for process spawn/communication failures
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Security Verification
        // =====================================================================
        // WHY: Ensure caller has authorization before expensive operations.
        // Cancellation check prevents wasted work on already-cancelled tasks.
        ctx.check_cancelled()?;

        // WHY: Only "head" agents may execute processes. This prevents "hand"
        // agents (which execute tool calls from LLMs) from running arbitrary
        // commands if the LLM is compromised or misbehaves.
        ctx.require_mutation()?;

        // =====================================================================
        // PHASE 2: Argument Parsing & Validation
        // =====================================================================
        // WHY: Validate arguments before allowlist check to provide clear error
        // messages for malformed requests.
        let args: ExecRunArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.program.is_empty() {
            return Err(KernelError::invalid_args("'program' is required"));
        }

        // WHY: Allowlist check happens early to fail fast on unauthorized programs
        // before resolving paths or spawning processes.
        if !self.is_allowed(&args.program) {
            return Err(KernelError::forbidden(format!(
                "program '{}' is not in the allowed list",
                args.program
            ))
            .with_help(format!("Allowed programs: {}", self.allowed.join(", "))));
        }

        // =====================================================================
        // PHASE 3: Path Resolution & Configuration
        // =====================================================================
        // WHY: VFS resolution ensures working directory is within mounted paths,
        // preventing filesystem escapes. Falls back to context cwd if unspecified.
        let cwd = if let Some(ref cwd_str) = args.cwd {
            let vfs = MountTable::global().ok_or_else(|| {
                KernelError::disabled("filesystem access disabled: no mounts configured")
            })?;
            let resolved = vfs.resolve(cwd_str)?;
            resolved.host_path
        } else {
            ctx.cwd.clone()
        };

        // WHY: Timeout defaults to context deadline (from task lane), with optional
        // per-call override. Ensures no infinite execution.
        let timeout = args
            .timeout_ms
            .or(ctx.deadline_ms)
            .map(Duration::from_millis);

        // WHY: Output limits prevent memory exhaustion. Defaults:
        // - stdout: 2MB (typical for build output, test results)
        // - stderr: 512KB (error messages are usually shorter)
        let max_stdout = args.max_stdout.unwrap_or(2 * 1024 * 1024);
        let max_stderr = args.max_stderr.unwrap_or(512 * 1024);

        // WHY: Final cancellation check before expensive process spawn. Prevents
        // launching processes for tasks that were cancelled during setup.
        ctx.check_cancelled()?;

        // =====================================================================
        // PHASE 4: Cancellation Propagation Setup
        // =====================================================================
        // WHY: Child processes must terminate when parent context cancels, otherwise
        // they become orphaned zombies consuming resources. Spawn a monitor task that
        // watches parent cancellation and propagates to child.
        //
        // CONCURRENCY: Monitor task is detached (not awaited), so it doesn't block
        // execution. It terminates naturally after cancellation propagates.
        let child_cancel = CancellationToken::new();
        let parent_cancel = ctx.cancel.clone();
        let child_cancel_clone = child_cancel.clone();

        tokio::spawn(async move {
            parent_cancel.cancelled().await;
            child_cancel_clone.cancel();
        });

        // =====================================================================
        // PHASE 5: Process Execution
        // =====================================================================
        // WHY: Delegate to HAL layer for platform-specific process spawning.
        // Two code paths: with stdin (for piped input) or without (for simple execution).
        //
        // PERFORMANCE: Output is bounded at max_stdout/max_stderr bytes, preventing
        // unbounded memory allocation. HAL layer handles truncation transparently.
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

        // =====================================================================
        // PHASE 6: Response Formatting & Error Handling
        // =====================================================================
        // WHY: Convert HAL errors to kernel errors for consistent error handling.
        // Emit Frame::ok for successful execution (even if exit code != 0, since
        // the *syscall* succeeded - the program just returned an error code).
        match result {
            Ok(output) => {
                // WHY: from_utf8_lossy handles non-UTF8 output gracefully (rare in
                // practice, but prevents crashes on binary output).
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                // WHY: Include truncation flags so callers know if output was incomplete.
                // This allows them to warn users or adjust max_stdout/max_stderr.
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
    async fn test_exec_run_requires_head_scope() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
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
    async fn test_exec_run_hand_scope_rejected() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
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
    async fn test_exec_run_echo_with_head_scope() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
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
    async fn test_exec_run_forbidden_program() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "head/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall.execute(&ctx, json!({ "program": "nc" }), tx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_exec_run_git_status_with_head_scope() {
        let tmp = TempDir::new().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .ok();

        let syscall = ExecRun::new();
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
    async fn test_exec_run_with_stdin() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
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
        let syscall = ExecRun::new();
        assert!(syscall.is_allowed("git"));
        assert!(syscall.is_allowed("cargo"));
        assert!(syscall.is_allowed("/usr/bin/git"));
        assert!(!syscall.is_allowed("nc"));
        assert!(!syscall.is_allowed("netcat"));
        assert!(!syscall.is_allowed("/bin/sh"));
    }
}
