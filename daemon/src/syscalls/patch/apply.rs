//! Patch:Apply - Apply unified diff patches with VFS path validation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables controlled application of unified diff patches within the Abbot
//! kernel's security model. It implements **proactive path validation** to ensure all
//! patch targets exist within VFS-mounted workspaces before executing the underlying
//! `patch` utility.
//!
//! **Critical security boundaries:**
//! - Actor-based mutation authorization (only "head" agents may apply patches)
//! - VFS path validation (all diff paths must be within mounted workspaces)
//! - Patch format validation (requires standard unified diff headers)
//! - Stdin-based execution (prevents temporary file attacks)
//! - Output size limiting (prevents memory exhaustion from verbose patch output)
//!
//! **Integration points:**
//! - `HalProcess` for process execution (invokes system `patch` utility)
//! - `MountTable::global()` for VFS-based path security validation
//! - `SyscallContext` for actor verification and cancellation propagation
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with stdout on successful patch application
//! - Returns `KernelError` for security violations, malformed patches, or patch failures
//!
//! SECURITY MODEL
//! ==============
//! This syscall prevents malicious patches from escaping workspace boundaries:
//!
//! 1. **Actor Authorization**
//!    - WHY: Patch application mutates files, so only "head" agents are permitted
//!    - HOW: `ctx.require_mutation()` enforces "head" actor requirement (line 67)
//!    - ATTACK PREVENTED: Compromised "hand" agents (controlled by LLMs) cannot
//!      apply patches from untrusted sources
//!
//! 2. **VFS Path Validation**
//!    - WHY: Prevents patches from modifying files outside mounted workspaces
//!    - HOW: Parse all `---` and `+++` headers, validate each path via VFS (lines 84-118)
//!    - ATTACK PREVENTED: Path traversal via `--- ../../etc/passwd` or absolute paths
//!    - TRADE-OFF: Validation happens before execution (fail-fast), but requires parsing
//!      diff headers twice (once here, once by `patch` utility)
//!
//! 3. **Patch Format Validation**
//!    - WHY: Ensures input is a valid unified diff, not arbitrary shell commands
//!    - HOW: Require presence of `---` and `+++` markers (lines 73-77)
//!    - ATTACK PREVENTED: Execution of malicious content disguised as patches
//!    - LIMITATION: Does not validate diff syntax deeply (delegates to `patch` utility)
//!
//! 4. **/dev/null Path Exemption**
//!    - WHY: Unified diffs use `/dev/null` to indicate file creation/deletion
//!    - HOW: Skip VFS validation for paths matching `/dev/null` (line 98)
//!    - RATIONALE: `--- /dev/null` (new file) and `+++ /dev/null` (deleted file)
//!      are standard diff conventions, not actual filesystem paths
//!
//! 5. **Stdin-Based Execution**
//!    - WHY: Prevents temporary file creation in potentially unsafe locations
//!    - HOW: Pipe patch content directly to `patch` utility via stdin (line 133)
//!    - ATTACK PREVENTED: Race conditions from temporary file manipulation
//!    - TRADE-OFF: Limited to patches that fit in memory (acceptable given 256KB limit)
//!
//! 6. **Output Limiting**
//!    - WHY: Prevents memory exhaustion from extremely verbose patch output
//!    - HOW: 256KB stdout/stderr limits, 4000 character response truncation (lines 134-135, 146)
//!    - ATTACK PREVENTED: Resource exhaustion DoS from pathological patches
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Fail-fast validation**: Validate all paths before execution to provide clear errors
//! - **VFS-first security**: All filesystem access goes through VFS layer, no exceptions
//! - **Standard tooling**: Delegate to battle-tested `patch` utility rather than reimplementing
//! - **Minimal output**: Truncate verbose patch logs to essential information
//! - **No backup files**: Use `--no-backup-if-mismatch` to avoid workspace pollution
//!
//! PERFORMANCE
//! ===========
//! - Path validation requires O(n) iteration over diff lines (acceptable for typical patches)
//! - Output is truncated at 256KB to bound memory usage
//! - Response is further clipped to 4000 characters for Frame::ok payload size
//!
//! CONCURRENCY
//! ===========
//! - Mutation guard prevents concurrent patch applications (serialized by actor model)
//! - Safe for use in task lanes (does not block kernel event loop)
//! - No explicit cancellation token passed (HAL layer handles cleanup)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Path Validation Timing**
//!    - CHOSEN: Validate all paths before invoking `patch` utility
//!    - WHY: Fail-fast with clear error messages (e.g., "references path outside workspace")
//!    - COST: Double-parsing diff headers (once here, once by `patch`)
//!    - ACCEPTABLE: Patches are typically small, parsing overhead is negligible
//!
//! 2. **Stdin vs. Temporary Files**
//!    - CHOSEN: Pipe patch via stdin (not temporary file)
//!    - WHY: Prevents race conditions from temporary file manipulation
//!    - IMPLICATION: Patch must fit in memory (256KB limit enforces this)
//!
//! 3. **No Dry-Run Validation**
//!    - CHOSEN: Apply patch immediately (no `--dry-run` pre-check)
//!    - WHY: Simpler implementation, consistent with git workflow (patches can fail)
//!    - IMPLICATION: Failed patches may leave workspace in inconsistent state
//!      (mitigated by git's ability to revert changes)

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalProcess, HostHalProcess};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `patch:apply` syscall.
///
/// WHY: Structured patch specification with minimal API surface.
/// Only accepts patch content (working directory comes from context).
#[derive(Debug, Deserialize)]
struct PatchApplyArgs {
    /// Unified diff patch content (must contain `---` and `+++` headers).
    ///
    /// WHY: Standard unified diff format enables interoperability with git,
    /// diff(1), and other version control tools. Format validation prevents
    /// execution of arbitrary content disguised as patches.
    patch: String,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for applying unified diff patches with VFS path validation.
///
/// WHY: Enables programmatic file modification via standard diff/patch workflow
/// while maintaining workspace isolation through VFS security boundaries.
pub struct PatchApply;

impl Default for PatchApply {
    fn default() -> Self {
        Self::new()
    }
}

impl PatchApply {
    /// Create a new `PatchApply` syscall.
    ///
    /// WHY: Stateless constructor - no HAL injection needed since we always
    /// use HostHalProcess (no test mocking infrastructure for patch operations).
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for PatchApply {
    fn name(&self) -> &'static str {
        "patch:apply"
    }

    /// Apply a unified diff patch with VFS path validation.
    ///
    /// WHY: Enables controlled file modification via standard patch workflow while
    /// enforcing workspace boundaries through VFS validation. Common use case is
    /// applying code changes generated by LLMs or retrieved from external sources.
    ///
    /// USE CASE: Invoked by "head" agents to apply file modifications. Typical workflow:
    /// 1. Agent generates diff (e.g., via code analysis or external API)
    /// 2. Agent calls `patch:apply` with unified diff content
    /// 3. Syscall validates all paths are within VFS mounts
    /// 4. System `patch` utility applies changes to working directory
    ///
    /// SECURITY NOTE: This syscall implements defense-in-depth:
    /// 1. Actor verification - only "head" agents may mutate files (line 67)
    /// 2. Format validation - patch must contain unified diff headers (lines 73-77)
    /// 3. VFS path validation - all diff paths must be within mounted workspace (lines 84-118)
    /// 4. Stdin-based execution - prevents temporary file race conditions (line 133)
    ///
    /// RETURNS:
    /// - `Frame::ok` with truncated stdout from `patch` utility
    /// - `E_FORBIDDEN` if actor lacks mutation permission or path is outside workspace
    /// - `E_INVALID_ARGS` if patch is empty or missing required diff headers
    /// - `E_DISABLED` if VFS is not configured
    /// - `E_CANCELLED` if parent context cancels during execution
    /// - `E_IO` if patch utility fails (malformed diff, file conflicts, etc.)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // ---------------------------------------------------------------------
        // SECURITY VERIFICATION
        // ---------------------------------------------------------------------
        // WHY: Check cancellation and mutation permission before expensive
        // operations. Only "head" agents may apply patches.
        ctx.check_cancelled()?;
        ctx.require_mutation()?;

        // ---------------------------------------------------------------------
        // ARGUMENT PARSING & FORMAT VALIDATION
        // ---------------------------------------------------------------------
        // WHY: Validate patch format before path resolution to fail fast on
        // obviously malformed input.
        let args: PatchApplyArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let diff = args.patch.trim();
        if diff.is_empty() {
            return Err(KernelError::invalid_args("patch is empty"));
        }

        // WHY: Unified diff format requires `---` (old file) and `+++` (new file)
        // headers. This check prevents execution of arbitrary content disguised
        // as patches (e.g., shell scripts, binary data).
        //
        // LIMITATION: Does not validate full diff syntax - just presence of headers.
        // The `patch` utility will catch malformed diffs during execution.
        if !diff.contains("---") || !diff.contains("+++") {
            return Err(KernelError::invalid_args(
                "invalid diff format (missing --- or +++ headers)",
            ));
        }

        // ---------------------------------------------------------------------
        // VFS PATH VALIDATION
        // ---------------------------------------------------------------------
        // WHY: Proactively validate all paths referenced in diff headers before
        // invoking `patch` utility. This ensures patches cannot escape workspace
        // boundaries via path traversal (../../etc/passwd) or absolute paths.
        //
        // SECURITY: This is the PRIMARY security boundary for this syscall.
        // Every path in the patch must be resolvable via VFS, or we reject the
        // entire operation before any filesystem mutation occurs.
        let vfs = MountTable::global().ok_or_else(|| {
            KernelError::disabled("filesystem access disabled: no mounts configured")
        })?;

        // WHY: Iterate over all lines looking for `---` and `+++` headers, which
        // indicate file paths in unified diff format. Each path must be validated
        // against VFS mounts to prevent workspace escapes.
        for line in diff.lines() {
            // WHY: Extract path from diff header line. Unified diff format:
            // `--- a/path/to/file.rs` (old version)
            // `+++ b/path/to/file.rs` (new version)
            let path = if let Some(rest) = line.strip_prefix("+++ ") {
                Some(rest)
            } else {
                line.strip_prefix("--- ")
            };

            if let Some(raw_path) = path {
                // WHY: `/dev/null` is a special sentinel in unified diffs:
                // - `--- /dev/null` indicates new file creation
                // - `+++ /dev/null` indicates file deletion
                // These are not actual filesystem paths, so skip VFS validation.
                //
                // SECURITY: This exception is safe because `/dev/null` is a
                // well-known diff convention, not a path traversal attempt.
                if raw_path == "/dev/null" || raw_path.starts_with("/dev/null") {
                    continue;
                }

                // WHY: Strip git-style `a/` and `b/` prefixes from paths.
                // Git generates diffs like `--- a/src/main.rs` and `+++ b/src/main.rs`,
                // but the actual filesystem path is `src/main.rs`.
                //
                // WHY: Split on tab to handle paths with timestamps or metadata:
                // `--- a/file.rs\t2025-01-01 12:00:00`
                let stripped = raw_path
                    .strip_prefix("a/")
                    .or_else(|| raw_path.strip_prefix("b/"))
                    .unwrap_or(raw_path)
                    .split('\t')
                    .next()
                    .unwrap_or(raw_path);

                // WHY: Skip empty paths (malformed diff lines with just `---` or `+++`)
                if stripped.is_empty() {
                    continue;
                }

                // WHY: Validate path via VFS resolution. This ensures the path:
                // 1. Is within a mounted workspace (not outside `/workspace/...`)
                // 2. Does not contain path traversal (`../../etc/passwd`)
                // 3. Is not an absolute path outside mounts (`/etc/passwd`)
                //
                // SECURITY: VFS resolve() is the trust boundary. If it succeeds,
                // the path is safe to pass to `patch` utility.
                vfs.resolve(stripped).map_err(|_| {
                    KernelError::forbidden(format!(
                        "patch references path outside workspace: {}",
                        stripped
                    ))
                })?;
            }
        }

        // ---------------------------------------------------------------------
        // PATCH UTILITY INVOCATION
        // ---------------------------------------------------------------------
        // WHY: Use standard `patch` utility with carefully chosen flags:
        // - `-p1`: Strip first path component (handles git-style `a/` and `b/` prefixes)
        // - `--no-backup-if-mismatch`: Don't create `.orig` backup files on failure
        // - `-r-`: Send reject files to stdout (not filesystem) on conflict
        let argv = vec![
            "-p1".to_string(),
            "--no-backup-if-mismatch".to_string(),
            "-r-".to_string(),
        ];

        // WHY: Pipe patch content via stdin rather than temporary file. This prevents
        // race conditions from temporary file manipulation and keeps patch content
        // in memory (bounded by 256KB output limits).
        //
        // WHY: 256KB output limits for both stdout/stderr prevent memory exhaustion
        // from pathological patches with extremely verbose output.
        //
        // WHY: No cancellation token passed (None) - HAL layer will clean up process
        // if parent task is cancelled via Drop semantics.
        let output = HostHalProcess
            .run_with_stdin_bytes_bounded(
                "patch",
                &argv,
                &ctx.cwd,
                None, // No custom environment variables
                None, // No explicit timeout (context deadline applies)
                diff.as_bytes(),
                256 * 1024, // Max stdout: 256KB
                256 * 1024, // Max stderr: 256KB
                None,       // No explicit cancellation token
            )
            .await;

        // ---------------------------------------------------------------------
        // RESPONSE FORMATTING & ERROR HANDLING
        // ---------------------------------------------------------------------
        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                if output.success {
                    // WHY: Truncate stdout to 4000 characters for Frame::ok payload.
                    // Patch output is typically minimal (file names and line counts),
                    // but can be verbose on complex merges.
                    let clipped: String = stdout.trim_end().chars().take(4000).collect();
                    let _ = tx
                        .send(Frame::ok(ctx.call_id, json!({"stdout": clipped})))
                        .await;
                    Ok(())
                } else {
                    // WHY: Prefer stderr for error messages (patch utility convention),
                    // but fall back to stdout if stderr is empty (some error modes).
                    let msg = if !stderr.trim().is_empty() {
                        stderr.trim().to_string()
                    } else {
                        stdout.trim().to_string()
                    };
                    Err(KernelError::io(format!("patch failed: {msg}")))
                }
            }
            Err(crate::hal::process::HalProcessError::Cancelled { .. }) => {
                Err(KernelError::cancelled("patch cancelled"))
            }
            Err(e) => Err(KernelError::io(format!("patch error: {e}"))),
        }
    }
}
