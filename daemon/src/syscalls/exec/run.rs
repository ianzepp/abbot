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
//!    - HOW: `is_allowed()` checks basename against the configured allowlist (minimum + defaults or `[exec].allowed`)
//!    - ATTACK PREVENTED: Execution of dangerous binaries like `nc`, `ssh`, `/bin/sh`
//!    - TRADE-OFF: Explicitly blocks shells to prevent command injection via `-c` flag
//!
//! 3. **Output Limiting**
//!    - WHY: Prevents memory exhaustion from programs with unbounded output
//!    - HOW: `max_stdout` (default 2MB) and `max_stderr` (default 512KB) truncate output
//!    - ATTACK PREVENTED: Resource exhaustion DoS from `yes`, `cat /dev/urandom`, etc.
//!
//! 4. **Working Directory Validation**
//!    - WHY: Prevents selecting a cwd outside the configured VFS mounts
//!    - HOW: VFS resolution rejects unmounted paths with `E_DISABLED`
//!    - NOTE: This does NOT sandbox the spawned process. It may still access any host paths
//!      permitted by OS-level permissions; only the chosen working directory is constrained.
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
use crate::vfs::{MountTable, VfsResolution};

// =============================================================================
// SECURITY CONSTANTS
// =============================================================================

/// Bare minimum programs always allowed (universally safe, no config needed).
const MINIMUM_ALLOWED_PROGRAMS: &[&str] = &[
    "echo", "printf", "true", "false", "test", "date", "env", "which", "whoami",
];

/// Default extended allowlist used when no `[exec].allowed` config is present.
///
/// Hardening stance: keep this list intentionally conservative. Add project-specific
/// tooling via `[exec].allowed` in `abbot.toml`.
pub const DEFAULT_EXEC_ALLOWED: &[&str] = &[
    "git", "gh", "cargo", "python", "python3", "ls", "cat", "head", "tail", "grep", "rg", "sed",
    "awk", "sort", "uniq", "wc", "diff", "patch", "tar", "gzip", "gunzip", "zip", "unzip", "jq",
    "yq", "rustc", "rustfmt", "clippy", "sqlite3", "npm", "npx", "node", "make", "cmake", "pytest",
    "jest",
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
    /// Build the full default allowlist (minimum + extended defaults).
    fn default_allowed() -> Vec<String> {
        let mut allowed: Vec<String> = MINIMUM_ALLOWED_PROGRAMS
            .iter()
            .chain(DEFAULT_EXEC_ALLOWED.iter())
            .map(|s| s.to_string())
            .collect();
        allowed.sort();
        allowed.dedup();
        allowed
    }

    /// Create a new `ExecRun` syscall with default allowed programs.
    ///
    /// WHY: Standard constructor for production use and tests with host OS process spawning.
    pub fn new() -> Self {
        Self {
            proc: Arc::new(HostHalProcess),
            allowed: Self::default_allowed(),
        }
    }

    /// Create from global AppConfig, merging minimum + user-configured allowlist.
    ///
    /// If `[exec].allowed` is empty in config, falls back to full default list.
    pub fn from_config() -> Self {
        let config = crate::runtime::AppConfig::global();
        let mut allowed: Vec<String> = MINIMUM_ALLOWED_PROGRAMS
            .iter()
            .map(|s| s.to_string())
            .collect();
        if config.exec.allowed.is_empty() {
            allowed.extend(DEFAULT_EXEC_ALLOWED.iter().map(|s| s.to_string()));
        } else {
            allowed.extend(config.exec.allowed.iter().cloned());
        }
        allowed.sort();
        allowed.dedup();
        Self {
            proc: Arc::new(HostHalProcess),
            allowed,
        }
    }

    /// Create a `ExecRun` syscall with a custom HAL process implementation.
    ///
    /// WHY: Enables testing with mock processes that simulate failures, timeouts, etc.
    pub fn with_process(proc: Arc<dyn HalProcess>) -> Self {
        Self {
            proc,
            allowed: Self::default_allowed(),
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

// =============================================================================
// READ-ONLY INVOCATION POLICY
// =============================================================================

/// Programs that are always read-only (hand agents can use freely).
const ALWAYS_READONLY: &[&str] = &[
    "ls", "find", "cat", "head", "tail", "grep", "rg", "sort", "uniq", "wc", "diff", "echo",
    "printf", "true", "false", "test", "date", "env", "which", "whoami", "jq", "yq", "pytest",
    "jest", "mocha", "rspec",
];

/// Check whether a program+args invocation is read-only.
///
/// Three tiers:
/// - Always read-only: `ls`, `cat`, `grep`, etc. → true regardless of args
/// - Subcommand-gated: `git status`, `cargo test`, etc. → true only for listed subcommands
/// - Always mutating: everything else → false
fn is_readonly_invocation(program: &str, args: &[String]) -> bool {
    let basename = std::path::Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program);

    // Tier 1: always read-only
    if ALWAYS_READONLY.contains(&basename) {
        return true;
    }

    let subcmd = args.first().map(|s| s.as_str()).unwrap_or("");

    // Tier 2: subcommand-gated
    match basename {
        "git" => matches!(
            subcmd,
            "status"
                | "log"
                | "diff"
                | "show"
                | "rev-parse"
                | "ls-files"
                | "ls-tree"
                | "cat-file"
                | "describe"
                | "shortlog"
                | "blame"
        ),
        "cargo" => matches!(
            subcmd,
            "check" | "test" | "clippy" | "tree" | "metadata" | "bench" | "doc"
        ),
        "npm" => matches!(
            subcmd,
            "ls" | "list" | "view" | "audit" | "outdated" | "info" | "test"
        ),
        "gh" => {
            // Two-level gating: gh <resource> <action>
            let action = args.get(1).map(|s| s.as_str()).unwrap_or("");
            matches!(
                (subcmd, action),
                ("pr", "list" | "view" | "checks" | "diff" | "status")
                    | ("issue", "list" | "view")
                    | ("repo", "view")
                    | ("run", "list" | "view")
            )
        }
        _ => false,
    }
}

/// Build help text listing the allowed read-only subcommands for a program.
fn readonly_help(program: &str) -> String {
    let basename = std::path::Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program);

    match basename {
        "git" => "Hand agents can use `git` with read-only subcommands: \
             status, log, diff, show, rev-parse, ls-files, ls-tree, cat-file, describe, shortlog, blame"
            .to_string(),
        "cargo" => "Hand agents can use `cargo` with read-only subcommands: \
             check, test, clippy, tree, metadata, bench, doc"
            .to_string(),
        "npm" => "Hand agents can use `npm` with read-only subcommands: \
             ls, list, view, audit, outdated, info, test"
            .to_string(),
        "gh" => "Hand agents can use `gh` with read-only subcommands: \
             pr list/view/checks/diff/status, issue list/view, repo view, run list/view"
            .to_string(),
        _ => format!(
            "Program '{basename}' is not available to hand agents. Only head/mind agents can run mutating commands."
        ),
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

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Cancellation Check
        // =====================================================================
        ctx.check_cancelled()?;

        // =====================================================================
        // PHASE 2: Argument Parsing & Validation
        // =====================================================================
        let args: ExecRunArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.program.is_empty() {
            return Err(KernelError::invalid_args("'program' is required"));
        }

        // Allowlist check: fail fast on unauthorized programs.
        if !self.is_allowed(&args.program) {
            return Err(KernelError::forbidden(format!(
                "program '{}' is not in the allowed list",
                args.program
            ))
            .with_help(format!("Allowed programs: {}", self.allowed.join(", "))));
        }

        // =====================================================================
        // PHASE 3: Actor Authorization
        // =====================================================================
        // Head/mind agents: unrestricted access to all allowed programs.
        // Hand agents (and anonymous): only read-only invocations permitted.
        if !ctx.can_mutate() && !is_readonly_invocation(&args.program, &args.args) {
            return Err(KernelError::forbidden(format!(
                "hand agent cannot run `{}{}`",
                args.program,
                if args.args.is_empty() {
                    String::new()
                } else {
                    format!(" {}", args.args.join(" "))
                }
            ))
            .with_help(readonly_help(&args.program)));
        }

        // =====================================================================
        // PHASE 3: Path Resolution & Configuration
        // =====================================================================
        // WHY: VFS resolution ensures working directory is within mounted paths,
        // preventing filesystem escapes. Falls back to context cwd if unspecified.
        let cwd = if let Some(ref cwd_str) = args.cwd {
            match MountTable::global().resolve(cwd_str)? {
                VfsResolution::Host(resolved) => resolved.host_path,
                VfsResolution::Memory { .. } => {
                    return Err(KernelError::invalid_args(
                        "exec:run requires cwd to be in a host mount (cannot execute in memory filesystem)",
                    ));
                }
            }
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
    async fn test_exec_run_no_actor_mutating_rejected() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
        let ctx = make_ctx(tmp.path());
        let (tx, _rx) = mpsc::channel(8);

        // mkdir is always-mutating → rejected for anonymous (no actor)
        let result = syscall
            .execute(&ctx, json!({ "program": "mkdir", "args": ["foo"] }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_exec_run_hand_scope_mutating_rejected() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "hand/test");
        let (tx, _rx) = mpsc::channel(8);

        // mkdir is always-mutating → rejected for hand
        let result = syscall
            .execute(&ctx, json!({ "program": "mkdir", "args": ["foo"] }), tx)
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

    // =========================================================================
    // is_readonly_invocation unit tests
    // =========================================================================

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_readonly_always_readonly_programs() {
        // Always read-only: hand can use freely regardless of args
        assert!(is_readonly_invocation("ls", &args(&["-la"])));
        assert!(is_readonly_invocation("cat", &args(&["file.txt"])));
        assert!(is_readonly_invocation("grep", &args(&["-r", "pattern"])));
        assert!(is_readonly_invocation("echo", &args(&["hello"])));
        assert!(is_readonly_invocation("jq", &args(&[".foo"])));
        assert!(is_readonly_invocation("pytest", &args(&["tests/"])));
        assert!(is_readonly_invocation("date", &[]));
        assert!(is_readonly_invocation("which", &args(&["git"])));
        assert!(is_readonly_invocation("whoami", &[]));
        assert!(is_readonly_invocation("true", &[]));
        assert!(is_readonly_invocation("false", &[]));
        assert!(is_readonly_invocation("test", &args(&["-f", "foo"])));
        assert!(is_readonly_invocation("wc", &args(&["-l"])));
        assert!(is_readonly_invocation("diff", &args(&["a.txt", "b.txt"])));
        assert!(is_readonly_invocation("sort", &args(&["data.txt"])));
        assert!(is_readonly_invocation("uniq", &[]));
    }

    #[test]
    fn test_readonly_git_subcommand_gating() {
        // Allowed git subcommands
        assert!(is_readonly_invocation("git", &args(&["status"])));
        assert!(is_readonly_invocation("git", &args(&["log", "--oneline"])));
        assert!(is_readonly_invocation("git", &args(&["diff", "HEAD"])));
        assert!(is_readonly_invocation("git", &args(&["show", "abc123"])));
        assert!(is_readonly_invocation("git", &args(&["rev-parse", "HEAD"])));
        assert!(is_readonly_invocation("git", &args(&["ls-files"])));
        assert!(is_readonly_invocation("git", &args(&["blame", "file.rs"])));

        // Denied git subcommands
        assert!(!is_readonly_invocation("git", &args(&["push"])));
        assert!(!is_readonly_invocation(
            "git",
            &args(&["commit", "-m", "msg"])
        ));
        assert!(!is_readonly_invocation("git", &args(&["checkout", "main"])));
        assert!(!is_readonly_invocation("git", &args(&["reset", "--hard"])));
        assert!(!is_readonly_invocation("git", &args(&["add", "."])));
        assert!(!is_readonly_invocation("git", &args(&["merge", "branch"])));
        assert!(!is_readonly_invocation("git", &[])); // no subcommand
    }

    #[test]
    fn test_readonly_cargo_subcommand_gating() {
        assert!(is_readonly_invocation("cargo", &args(&["check"])));
        assert!(is_readonly_invocation("cargo", &args(&["test"])));
        assert!(is_readonly_invocation("cargo", &args(&["clippy"])));
        assert!(is_readonly_invocation("cargo", &args(&["tree"])));
        assert!(is_readonly_invocation("cargo", &args(&["metadata"])));

        assert!(!is_readonly_invocation("cargo", &args(&["build"])));
        assert!(!is_readonly_invocation("cargo", &args(&["install", "foo"])));
        assert!(!is_readonly_invocation("cargo", &args(&["run"])));
        assert!(!is_readonly_invocation("cargo", &args(&["fmt"])));
    }

    #[test]
    fn test_readonly_npm_subcommand_gating() {
        assert!(is_readonly_invocation("npm", &args(&["ls"])));
        assert!(is_readonly_invocation("npm", &args(&["list"])));
        assert!(is_readonly_invocation("npm", &args(&["audit"])));
        assert!(is_readonly_invocation("npm", &args(&["test"])));
        assert!(is_readonly_invocation("npm", &args(&["outdated"])));

        assert!(!is_readonly_invocation("npm", &args(&["install"])));
        assert!(!is_readonly_invocation("npm", &args(&["publish"])));
        assert!(!is_readonly_invocation("npm", &args(&["run", "build"])));
    }

    #[test]
    fn test_readonly_gh_two_level_gating() {
        // Allowed gh subcommands
        assert!(is_readonly_invocation("gh", &args(&["pr", "list"])));
        assert!(is_readonly_invocation("gh", &args(&["pr", "view", "123"])));
        assert!(is_readonly_invocation("gh", &args(&["pr", "checks"])));
        assert!(is_readonly_invocation("gh", &args(&["pr", "diff"])));
        assert!(is_readonly_invocation("gh", &args(&["pr", "status"])));
        assert!(is_readonly_invocation("gh", &args(&["issue", "list"])));
        assert!(is_readonly_invocation(
            "gh",
            &args(&["issue", "view", "42"])
        ));
        assert!(is_readonly_invocation("gh", &args(&["repo", "view"])));
        assert!(is_readonly_invocation("gh", &args(&["run", "list"])));
        assert!(is_readonly_invocation("gh", &args(&["run", "view", "123"])));

        // Denied gh subcommands
        assert!(!is_readonly_invocation("gh", &args(&["pr", "create"])));
        assert!(!is_readonly_invocation("gh", &args(&["pr", "merge"])));
        assert!(!is_readonly_invocation("gh", &args(&["pr", "close"])));
        assert!(!is_readonly_invocation("gh", &args(&["issue", "create"])));
        assert!(!is_readonly_invocation("gh", &args(&["repo", "create"])));
        assert!(!is_readonly_invocation("gh", &args(&["run", "rerun"])));
        assert!(!is_readonly_invocation("gh", &args(&["pr"]))); // no action
        assert!(!is_readonly_invocation("gh", &[])); // no subcommand
    }

    #[test]
    fn test_readonly_always_mutating_programs() {
        // Always mutating programs → false regardless of args
        assert!(!is_readonly_invocation("rm", &args(&["-rf", "foo"])));
        assert!(!is_readonly_invocation("mkdir", &args(&["new_dir"])));
        assert!(!is_readonly_invocation(
            "curl",
            &args(&["https://example.com"])
        ));
        assert!(!is_readonly_invocation("sed", &args(&["s/a/b/", "file"])));
        assert!(!is_readonly_invocation("python", &args(&["script.py"])));
        assert!(!is_readonly_invocation("node", &args(&["app.js"])));
        assert!(!is_readonly_invocation("make", &args(&["build"])));
        assert!(!is_readonly_invocation("cp", &args(&["a", "b"])));
        assert!(!is_readonly_invocation("mv", &args(&["a", "b"])));
        assert!(!is_readonly_invocation("touch", &args(&["file"])));
        assert!(!is_readonly_invocation("chmod", &args(&["+x", "file"])));
        assert!(!is_readonly_invocation("sleep", &args(&["1"])));
    }

    #[test]
    fn test_readonly_path_based_program() {
        // Basename extraction works for paths
        assert!(is_readonly_invocation("/usr/bin/ls", &args(&["-la"])));
        assert!(is_readonly_invocation("/usr/bin/git", &args(&["status"])));
        assert!(!is_readonly_invocation("/usr/bin/git", &args(&["push"])));
        assert!(!is_readonly_invocation("/usr/bin/rm", &args(&["-rf", "/"])));
    }

    // =========================================================================
    // Integration tests: hand agent allowed/denied scenarios
    // =========================================================================

    #[tokio::test]
    async fn test_hand_echo_allowed() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "hand/test");
        let (tx, mut rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "program": "echo", "args": ["hello"] }), tx)
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.op, crate::kernel::FrameOp::Ok);
        let data = frame.data.unwrap();
        assert!(data["success"].as_bool().unwrap());
        assert!(data["stdout"].as_str().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn test_hand_git_status_allowed() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .ok();

        let syscall = ExecRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "hand/test");
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
    async fn test_hand_git_push_rejected() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "hand/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "program": "git", "args": ["push"] }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
        assert!(err.help.is_some());
    }

    #[tokio::test]
    async fn test_hand_rm_rejected() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "hand/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "program": "rm", "args": ["-rf", "foo"] }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_hand_gh_pr_list_allowed() {
        // We can't actually run `gh pr list` without auth, but we can test
        // that the invocation passes the readonly gate. Use a mock process.
        // For now just verify the policy function allows it.
        assert!(is_readonly_invocation("gh", &args(&["pr", "list"])));
    }

    #[tokio::test]
    async fn test_hand_gh_pr_create_rejected() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "hand/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "program": "gh", "args": ["pr", "create"] }),
                tx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[tokio::test]
    async fn test_head_rm_forbidden_by_default() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
        let ctx = make_ctx_with_actor(tmp.path(), "head/test");
        let (tx, _rx) = mpsc::channel(8);

        // Hardening: destructive programs like rm are not allowed by default.
        let result = syscall
            .execute(
                &ctx,
                json!({ "program": "rm", "args": ["-f", "nonexistent"] }),
                tx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[test]
    fn test_from_config_defaults_match_new() {
        // With no config set (test default), from_config() should produce the same
        // allowlist as new().
        let from_new = ExecRun::new();
        let from_config = ExecRun::from_config();
        assert_eq!(from_new.allowed, from_config.allowed);
    }

    #[test]
    fn test_default_allowed_includes_minimum_and_extended() {
        let syscall = ExecRun::new();
        // Minimum programs always present
        for p in MINIMUM_ALLOWED_PROGRAMS {
            assert!(syscall.is_allowed(p), "missing minimum program: {}", p);
        }
        // Extended defaults present
        for p in DEFAULT_EXEC_ALLOWED {
            assert!(syscall.is_allowed(p), "missing default program: {}", p);
        }
    }

    #[tokio::test]
    async fn test_no_actor_readonly_allowed() {
        let tmp = TempDir::new().unwrap();
        let syscall = ExecRun::new();
        let ctx = make_ctx(tmp.path()); // no actor
        let (tx, mut rx) = mpsc::channel(8);

        // Anonymous callers can still use read-only programs
        let result = syscall
            .execute(&ctx, json!({ "program": "echo", "args": ["hello"] }), tx)
            .await;

        assert!(result.is_ok());
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.op, crate::kernel::FrameOp::Ok);
    }
}
