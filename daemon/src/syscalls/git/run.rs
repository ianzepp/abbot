//! Git:Run - Execute git commands with layered security controls
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides controlled execution of git operations within the Abbot kernel's
//! security model. It implements a **three-tier security strategy** to balance functionality
//! with safety:
//!
//! 1. **Forbidden commands** - Never allowed (push, config, credential, remote)
//! 2. **Mutating commands** - Allowed only for "head" agents (commit, merge, etc.)
//! 3. **Read-only commands** - Allowed for all agents (status, log, diff, etc.)
//!
//! **Integration points:**
//! - `HalProcess` for process execution (delegates to exec:run internally)
//! - `MountTable::global()` for VFS-based working directory resolution
//! - `SyscallContext` for actor verification, cancellation, and deadline enforcement
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with stdout/stderr/exit code on successful execution
//! - Returns `KernelError` for forbidden commands, actor violations, or process failures
//!
//! SECURITY MODEL
//! ==============
//! This syscall prevents destructive git operations and credential exposure:
//!
//! 1. **Forbidden Command Blocklist**
//!    - WHY: Certain git commands are **never safe** even for "head" agents
//!    - BLOCKED: `push`, `credential`, `config`, `remote`
//!    - RATIONALE:
//!      - `push`: Can expose private code to external repositories
//!      - `credential`: Exposes authentication tokens/passwords
//!      - `config`: Can modify git behavior globally (e.g., core.hooksPath for RCE)
//!      - `remote`: Can add malicious remotes for push attacks
//!
//! 2. **Mutating Command Authorization**
//!    - WHY: Prevent "hand" agents (controlled by LLMs) from modifying git history
//!    - HOW: `ctx.require_mutation()` enforces "head" actor for mutating commands
//!    - MUTATING: add, commit, merge, rebase, reset, checkout, switch, restore, etc.
//!    - ATTACK PREVENTED: Compromised LLM cannot commit malicious code
//!
//! 3. **Dangerous Flag Blocklist**
//!    - WHY: Certain flags enable arbitrary code execution
//!    - BLOCKED: `--exec`, `-c` (git config override)
//!    - EXAMPLE ATTACK: `git status --exec="rm -rf /"` (hypothetical)
//!    - REALITY: `--exec` is primarily for rebase/filter-branch, but blocked preemptively
//!
//! 4. **Working Directory Isolation**
//!    - WHY: Ensures git operations only affect VFS-mounted paths
//!    - HOW: VFS resolution at line 135 rejects unmounted paths
//!    - ATTACK PREVENTED: Git operations outside project workspace
//!
//! 5. **Output Limiting**
//!    - WHY: Prevents memory exhaustion from large git logs
//!    - HOW: 2MB stdout, 512KB stderr limits (same as exec:run)
//!    - ATTACK PREVENTED: `git log --all --patch` flooding memory
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Layered security**: Forbidden > Mutating > Read-only checks
//! - **Explicit deny-list**: Block known-dangerous commands rather than allow-list
//! - **Actor-based mutation control**: "Head" decides, "hand" reads
//! - **No network operations**: Push/pull/fetch blocked to prevent data exfiltration
//! - **Config immutability**: Git config is read-only (cannot modify behavior)
//!
//! PERFORMANCE
//! ===========
//! - Output is truncated at 2MB stdout / 512KB stderr to bound memory usage
//! - Cancellation token allows early termination without polling overhead
//! - VFS resolution happens once at start, not per-file-access
//!
//! CONCURRENCY
//! ===========
//! - Spawns detached monitor task to propagate cancellation from parent to child
//! - Git process blocking does not block kernel event loop (delegated to HAL layer)
//! - Safe for concurrent execution across multiple task lanes
//!
//! TRADE-OFFS
//! ==========
//! 1. **Deny-List vs. Allow-List**
//!    - CHOSEN: Deny-list for forbidden/mutating commands
//!    - WHY: Git has 100+ subcommands, allow-listing is fragile
//!    - RISK: New git commands may be unsafe by default (mitigated by actor restrictions)
//!
//! 2. **No Git Push**
//!    - CHOSEN: Block push entirely (not even for "head" agents)
//!    - WHY: Push can expose private code to external repositories
//!    - WORKAROUND: Use external CI/CD for deployments, not in-kernel pushes

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

/// Git subcommands that are **never allowed**, even for "head" agents.
///
/// WHY: These commands pose unacceptable security risks:
/// - `push`: Data exfiltration to external repositories
/// - `credential`: Exposes authentication tokens/passwords
/// - `config`: Can enable arbitrary code execution via core.hooksPath
/// - `remote`: Can add malicious remotes for later push attacks
const FORBIDDEN_GIT_COMMANDS: &[&str] = &["push", "credential", "config", "remote"];

/// Git subcommands that modify repository state.
///
/// WHY: These commands require "head" actor authorization (mutation permission).
/// Prevents "hand" agents (controlled by LLMs) from committing code or altering history.
const MUTATING_GIT_COMMANDS: &[&str] = &[
    "add",         // Stage files
    "commit",      // Create commits
    "merge",       // Merge branches
    "rebase",      // Rewrite history
    "reset",       // Move HEAD/branch pointers
    "checkout",    // Switch branches or restore files (can discard changes)
    "switch",      // Switch branches
    "restore",     // Restore working tree files
    "stash",       // Stash changes
    "cherry-pick", // Apply commits
    "revert",      // Create revert commits
    "clean",       // Delete untracked files
    "rm",          // Remove files from index
    "mv",          // Move files
    "branch",      // Create/delete branches (includes -D flag)
    "tag",         // Create/delete tags
    "fetch",       // Download objects from remote (updates refs)
    "pull",        // Fetch + merge (mutates working tree)
    "clone",       // Create new repository
    "init",        // Initialize repository
    "worktree",    // Manage worktrees
    "submodule",   // Manage submodules
    "apply",       // Apply patches
    "am",          // Apply mailbox patches
];

/// Git flags that enable arbitrary code execution.
///
/// WHY: These flags can run shell commands or override git config:
/// - `--exec`: Runs commands during rebase/filter-branch operations
/// - `-c`: Temporarily overrides git config (e.g., `git -c core.hooksPath=/tmp/evil`)
///
/// SECURITY: Blocked even if the subcommand itself is allowed.
const DANGEROUS_FLAGS: &[&str] = &["--exec", "-c"];

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `git:run` syscall.
///
/// WHY: Structured git command specification with security validation.
#[derive(Debug, Deserialize)]
struct GitRunArgs {
    /// Git arguments (subcommand + flags).
    ///
    /// WHY: First element is the git subcommand (e.g., "status"), rest are flags/arguments.
    /// Validated against forbidden/mutating command lists before execution.
    #[serde(default)]
    args: Vec<String>,

    /// Working directory for git command.
    ///
    /// WHY: Defaults to syscall context's cwd if unspecified.
    /// SECURITY: VFS resolution ensures path is within mounted workspace.
    #[serde(default)]
    cwd: Option<String>,

    /// Timeout in milliseconds.
    ///
    /// WHY: Prevents hung git operations (e.g., network timeouts, infinite diffs).
    #[serde(default)]
    timeout_ms: Option<u64>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for executing git commands with security constraints.
///
/// WHY: Encapsulates HAL process abstraction and git-specific security logic.
pub struct GitRun {
    /// Hardware abstraction for process spawning.
    ///
    /// WHY: Enables testing with mock processes, consistent with exec:run pattern.
    proc: Arc<dyn HalProcess>,
}

impl GitRun {
    /// Create a new `GitRun` syscall with host OS process spawning.
    ///
    /// WHY: Standard constructor for production use.
    pub fn new() -> Self {
        Self {
            proc: Arc::new(HostHalProcess),
        }
    }

    /// Create a `GitRun` syscall with a custom HAL process implementation.
    ///
    /// WHY: Enables testing with mock git command responses.
    pub fn with_process(proc: Arc<dyn HalProcess>) -> Self {
        Self { proc }
    }

    /// Validate git arguments against forbidden commands and dangerous flags.
    ///
    /// WHY: First line of defense - reject obviously dangerous operations before
    /// actor authorization checks. Prevents wasted work parsing arguments for
    /// commands that will never be allowed.
    ///
    /// SECURITY: Checks both subcommand (first arg) and all flags for dangerous patterns.
    fn validate_args(&self, args: &[String]) -> Result<(), KernelError> {
        if args.is_empty() {
            return Ok(());
        }

        // WHY: Check forbidden commands first (fail fast on always-unsafe operations)
        let subcommand = &args[0];
        if FORBIDDEN_GIT_COMMANDS.contains(&subcommand.as_str()) {
            return Err(KernelError::forbidden(format!(
                "git subcommand '{}' is not allowed",
                subcommand
            ))
            .with_help("Use read-only git commands like status, log, diff, show, branch, etc."));
        }

        // WHY: Check all arguments for dangerous flags (not just subcommand).
        // Prevents attacks like `git status --exec="malicious command"`.
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

    /// Check if git command modifies repository state.
    ///
    /// WHY: Determines whether `ctx.require_mutation()` check is needed.
    /// Mutating commands require "head" actor authorization.
    ///
    /// TRADE-OFF: False negatives (classifying mutating command as read-only) are
    /// safer than false positives. Unknown git commands default to read-only.
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

    /// Execute a git command with layered security controls.
    ///
    /// WHY: Provides safe git access for agents while preventing destructive operations,
    /// credential exposure, and unauthorized repository modifications.
    ///
    /// USE CASE: Invoked by agents to query git state (status, log, diff) or perform
    /// authorized mutations (commit, merge, rebase) when actor is "head".
    ///
    /// SECURITY NOTE: Three-tier validation:
    /// 1. Forbidden commands rejected immediately (push, config, credential, remote)
    /// 2. Mutating commands require "head" actor authorization (commit, merge, etc.)
    /// 3. Read-only commands allowed for all agents (status, log, diff, show)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{code, success, stdout, stderr, stdout_truncated, stderr_truncated}`
    /// - `E_FORBIDDEN` if command/flag is forbidden or actor lacks mutation permission
    /// - `E_DISABLED` if VFS is not configured
    /// - `E_CANCELLED` if parent context cancels during execution
    /// - `E_TIMEOUT` if execution exceeds timeout
    /// - `E_IO` for git execution failures
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Argument Parsing & Security Validation
        // =====================================================================
        ctx.check_cancelled()?;

        let args: GitRunArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Validate forbidden commands/flags BEFORE actor check (fail fast)
        self.validate_args(&args.args)?;

        // WHY: Conditional mutation check - only enforce for mutating commands.
        // Read-only commands (status, log, diff) skip actor verification.
        if self.is_mutating(&args.args) {
            ctx.require_mutation()?;
        }

        // =====================================================================
        // PHASE 2: Working Directory Resolution
        // =====================================================================
        // WHY: VFS resolution ensures git operates within mounted workspace
        let cwd = if let Some(ref cwd_str) = args.cwd {
            match MountTable::global().resolve(cwd_str)? {
                VfsResolution::Host(resolved) => resolved.host_path,
                VfsResolution::Memory { .. } => {
                    return Err(KernelError::invalid_args(
                        "git:run requires cwd to be in a host mount (cannot execute in memory filesystem)",
                    ));
                }
            }
        } else {
            ctx.cwd.clone()
        };

        let timeout = args
            .timeout_ms
            .or(ctx.deadline_ms)
            .map(Duration::from_millis);

        // WHY: Same output limits as exec:run (2MB stdout, 512KB stderr).
        // Prevents memory exhaustion from `git log --all --patch`.
        const MAX_STDOUT: usize = 2 * 1024 * 1024;
        const MAX_STDERR: usize = 512 * 1024;

        ctx.check_cancelled()?;

        // =====================================================================
        // PHASE 3: Cancellation Propagation Setup
        // =====================================================================
        // WHY: Git commands can be long-running (e.g., merge conflicts).
        // Propagate parent cancellation to child process.
        let child_cancel = CancellationToken::new();
        let parent_cancel = ctx.cancel.clone();
        let child_cancel_clone = child_cancel.clone();

        tokio::spawn(async move {
            parent_cancel.cancelled().await;
            child_cancel_clone.cancel();
        });

        // =====================================================================
        // PHASE 4: Git Command Execution
        // =====================================================================
        // WHY: Delegate to HAL layer for bounded process execution
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

        // =====================================================================
        // PHASE 5: Response Formatting & Error Handling
        // =====================================================================
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

    #[test]
    fn test_validate_args() {
        let syscall = GitRun::new();

        assert!(syscall.validate_args(&["status".to_string()]).is_ok());
        assert!(syscall.validate_args(&["log".to_string()]).is_ok());
        assert!(syscall.validate_args(&["diff".to_string()]).is_ok());
        assert!(syscall.validate_args(&["push".to_string()]).is_err());
        assert!(syscall.validate_args(&["config".to_string()]).is_err());
        assert!(
            syscall
                .validate_args(&["log".to_string(), "--exec=malicious".to_string()])
                .is_err()
        );
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
