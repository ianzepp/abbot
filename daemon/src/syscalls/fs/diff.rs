//! Fs:Diff - Generate unified diffs between files with VFS validation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall generates unified diffs between two files within VFS-mounted workspaces by
//! invoking the system `diff` utility. It implements **dual VFS resolution** (both paths
//! validated separately) and output limiting to prevent memory exhaustion.
//!
//! **Integration points:**
//! - `HalProcess` for process execution (invokes system `diff` utility)
//! - `VfsSource` for VFS resolution (validates both input paths)
//! - `SyscallContext` for cancellation propagation and working directory
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{ "diff": "..." }` payload (unified diff format)
//! - Empty string if files are identical (diff exit code 0)
//! - Returns `E_DISABLED` for VFS unavailable, `E_CANCELLED` for task cancellation, `E_IO` for diff errors
//!
//! SECURITY MODEL
//! ==============
//! 1. **Dual VFS Path Resolution** (Primary Security Boundary)
//!    - WHY: Prevents diffing files outside mounted workspaces
//!    - HOW: Separately resolve `a` and `b` paths via VFS (lines 123-124)
//!    - ATTACK PREVENTED: Path traversal (`../../etc/passwd`), absolute paths (`/etc/shadow`)
//!    - RATIONALE: Both paths must be within mounted workspace (no cross-mount diffs)
//!
//! 2. **Read-only Operation** (No Mutation Guard)
//!    - WHY: Diff does not modify files, so all agents may diff
//!    - HOW: No `ctx.require_mutation()` check (unlike fs:write)
//!    - RATIONALE: Agents need diff output for code understanding and patch generation
//!
//! 3. **Output Limiting** (Memory Exhaustion Prevention)
//!    - WHY: Prevents Frame payload explosion from massive diffs
//!    - HOW: 512KB stdout limit (HAL layer) + 20KB clipping (line 160)
//!    - ATTACK PREVENTED: Resource exhaustion from diffing huge files or deeply nested changes
//!    - TRADE-OFF: Large diffs are truncated (acceptable for preview purposes)
//!
//! 4. **Cancellation Propagation**
//!    - WHY: Long-running diffs (e.g., binary files) should terminate when task cancels
//!    - HOW: Pass `ctx.cancel` token to HAL process execution (line 141)
//!    - ATTACK PREVENTED: Orphaned diff processes consuming resources
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Standard tooling**: Delegate to battle-tested `diff` utility (not reimplemented)
//! - **Unified diff format**: Compatible with `patch:apply`, git, and other VCS tools
//! - **Context lines control**: Configurable context (default 3 lines) via `-U` flag
//! - **Output truncation**: Clip at 20KB for Frame payload size (reasonable for typical diffs)
//! - **Empty diff handling**: Distinguish "no differences" (empty string) from errors (E_IO)
//!
//! PERFORMANCE
//! ===========
//! - Delegates to OS-optimized `diff` utility (efficient binary comparison)
//! - Output is bounded at 512KB (HAL layer) then clipped to 20KB (Frame layer)
//! - Cancellation token allows early termination without polling overhead
//!
//! CONCURRENCY
//! ===========
//! - Spawns detached diff process (does not block kernel event loop)
//! - Cancellation token propagates from parent task to child process
//! - Safe for concurrent execution across multiple task lanes
//!
//! TRADE-OFFS
//! ==========
//! 1. **System Diff vs. Pure Rust**
//!    - CHOSEN: Invoke system `diff` utility via HAL process layer
//!    - WHY: Leverage battle-tested, OS-optimized diff implementation
//!    - RISK: Dependency on system binary availability (acceptable for Unix-like systems)
//!    - ALTERNATIVE: Pure Rust diff library (more portable, less optimized)
//!
//! 2. **Output Truncation vs. Streaming**
//!    - CHOSEN: Truncate at 20KB (not streaming via Frame::event)
//!    - WHY: Simpler implementation, bounded Frame payload size
//!    - IMPLICATION: Large diffs (>20KB) are incomplete (must refine comparison or use git)
//!
//! 3. **Unified Diff Only**
//!    - CHOSEN: Only support unified diff format (not side-by-side, context, etc.)
//!    - WHY: Unified format is most compatible with patch:apply and VCS tools
//!    - IMPLICATION: No customization of diff format (acceptable for programmatic use)

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalProcess, HostHalProcess};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `fs:diff` syscall.
///
/// WHY: Minimal API surface - only paths and context lines. Unified diff format
/// is hardcoded for consistency with patch:apply and VCS tools.
#[derive(Debug, Deserialize)]
struct FsDiffArgs {
    /// Virtual path to first file (resolved via VFS).
    ///
    /// WHY: VFS resolution ensures path is within mounted workspace.
    a: String,

    /// Virtual path to second file (resolved via VFS).
    ///
    /// WHY: VFS resolution ensures path is within mounted workspace.
    b: String,

    /// Number of context lines around changes (default 3).
    ///
    /// WHY: Configurable context allows balancing between verbosity and context.
    /// Default 3 matches git diff convention.
    #[serde(default)]
    context_lines: Option<u32>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for generating unified diffs with VFS path validation.
///
/// WHY: Provides secure diff generation for agents while maintaining workspace
/// isolation through dual VFS resolution.
pub struct FsDiff {
    /// VFS resolution strategy.
    ///
    /// WHY: Enables dependency injection for testing (Global, Disabled, or custom Table).
    vfs: VfsSource,
}

impl FsDiff {
    /// Create a new `FsDiff` syscall with global VFS.
    ///
    /// WHY: Standard constructor for production use with global mount table.
    pub fn new() -> Self {
        Self {
            vfs: VfsSource::Global,
        }
    }

    /// Create a new `FsDiff` syscall with an injected VFS mount table.
    #[allow(dead_code)]
    pub fn with_vfs(vfs: Arc<MountTable>) -> Self {
        Self {
            vfs: VfsSource::Table(vfs),
        }
    }
}

impl Default for FsDiff {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsDiff {
    fn name(&self) -> &'static str {
        "fs:diff"
    }

    /// Generate unified diff between two files with VFS validation.
    ///
    /// WHY: Provides secure diff generation for agents while maintaining workspace
    /// isolation. Supports configurable context lines for diff verbosity control.
    ///
    /// USE CASE: Invoked by agents to compare file versions, generate patches, or
    /// understand code changes. Common patterns:
    /// - Compare files: `{ "a": "src/main.rs", "b": "src/main.rs.bak" }`
    /// - More context: `{ "a": "file1.txt", "b": "file2.txt", "context_lines": 5 }`
    ///
    /// SECURITY NOTE: Dual VFS resolution (lines 123-124) validates both paths
    /// separately. Both must be within mounted workspace.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{ "diff": "..." }` payload (unified diff format, clipped to 20KB)
    /// - Empty diff ("") if files are identical (diff exit code 0)
    /// - `E_DISABLED` if VFS is not configured
    /// - `E_FORBIDDEN` if either path is outside mounted workspace
    /// - `E_INVALID_ARGS` if paths are empty
    /// - `E_CANCELLED` if parent context cancels during execution
    /// - `E_IO` for diff utility errors (file not found, permission denied, etc.)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // ---------------------------------------------------------------------
        // PHASE 1: Argument Parsing & Validation
        // ---------------------------------------------------------------------
        ctx.check_cancelled()?;

        let args: FsDiffArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.a.trim().is_empty() || args.b.trim().is_empty() {
            return Err(KernelError::invalid_args("both 'a' and 'b' paths are required"));
        }

        // ---------------------------------------------------------------------
        // PHASE 2: Dual VFS Path Resolution (SECURITY BOUNDARY)
        // ---------------------------------------------------------------------
        // WHY: Both paths must be validated separately via VFS. Cannot diff files
        // across mount boundaries or outside workspace.
        let vfs = match &self.vfs {
            VfsSource::Disabled => {
                return Err(KernelError::disabled(
                    "filesystem access disabled: no mounts configured",
                ));
            }
            VfsSource::Global => MountTable::global().ok_or_else(|| {
                KernelError::disabled("filesystem access disabled: no mounts configured")
            })?,
            VfsSource::Table(t) => t.as_ref(),
        };

        // WHY: Resolve both paths through VFS trust boundary
        let resolved_a = vfs.resolve(&args.a)?;
        let resolved_b = vfs.resolve(&args.b)?;

        // ---------------------------------------------------------------------
        // PHASE 3: Diff Utility Invocation
        // ---------------------------------------------------------------------
        // WHY: Use unified diff format (-U flag) with configurable context.
        // Default 3 lines matches git diff convention.
        let context = args.context_lines.unwrap_or(3);
        let argv = vec![
            "-U".to_string(),
            context.to_string(),
            resolved_a.host_path.to_string_lossy().to_string(),
            resolved_b.host_path.to_string_lossy().to_string(),
        ];

        // WHY: Bounded output (512KB stdout, 256KB stderr) prevents memory exhaustion.
        // Cancellation token allows early termination.
        let out = HostHalProcess::default()
            .run_bounded(
                "diff",
                &argv,
                &ctx.cwd,
                None,                      // No custom environment
                None,                      // No explicit timeout (context deadline applies)
                512 * 1024,                // Max stdout: 512KB
                256 * 1024,                // Max stderr: 256KB
                Some(ctx.cancel.clone()),  // Propagate cancellation
            )
            .await;

        // ---------------------------------------------------------------------
        // PHASE 4: Response Formatting & Error Handling
        // ---------------------------------------------------------------------
        match out {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                // WHY: Diff exit code 0 means no differences (empty stdout).
                // Non-zero with empty stdout indicates error (read stderr).
                if stdout.trim().is_empty() {
                    if output.code == 0 {
                        // WHY: Empty diff means files are identical
                        let _ = tx
                            .send(Frame::ok(ctx.call_id, json!({"diff": ""})))
                            .await;
                    } else {
                        // WHY: Non-zero exit with empty stdout is an error (e.g., file not found)
                        return Err(KernelError::io(stderr.trim().to_string()));
                    }
                } else {
                    // WHY: Clip stdout to 20KB for Frame payload size control.
                    // Large diffs (refactors, generated files) exceed this limit.
                    let clipped: String = stdout.chars().take(20_000).collect();
                    let _ = tx
                        .send(Frame::ok(ctx.call_id, json!({"diff": clipped})))
                        .await;
                }
                Ok(())
            }
            Err(crate::hal::process::HalProcessError::Cancelled { .. }) => {
                Err(KernelError::cancelled("diff cancelled"))
            }
            Err(e) => Err(KernelError::io(format!("diff error: {e}"))),
        }
    }
}
