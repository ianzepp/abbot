//! Fs:List - List directory contents with VFS path validation and glob filtering
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides directory listing within VFS-mounted workspaces with optional glob
//! pattern filtering and recursive traversal. It implements result truncation to prevent
//! memory exhaustion from large directory trees.
//!
//! **Integration points:**
//! - `VfsSource` for VFS resolution (validates path within mounts)
//! - `walkdir` crate for recursive directory traversal with depth control
//! - `globset` crate for filename pattern matching
//! - `SyscallContext` for cancellation propagation
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{ "matches": [...], "truncated": bool }` payload
//! - Returns `E_NOT_FOUND` for missing directories, `E_DISABLED` for VFS unavailable
//!
//! SECURITY MODEL
//! ==============
//! 1. **VFS Path Resolution** (Primary Security Boundary)
//!    - WHY: Prevents listing directories outside mounted workspaces
//!    - HOW: `vfs.resolve(&path)` validates path against mount table (line 134)
//!    - ATTACK PREVENTED: Path traversal (`../../etc`), absolute paths (`/etc`)
//!
//! 2. **Result Limiting** (Resource Exhaustion Prevention)
//!    - WHY: Prevents memory exhaustion from massive directory trees
//!    - HOW: `max_results` parameter (default 1000, cap 5000) truncates output (line 111)
//!    - ATTACK PREVENTED: Resource exhaustion DoS from recursive listing of huge trees
//!    - TRADE-OFF: Large directories require multiple queries with different filters
//!
//! 3. **Depth Control** (Recursive vs. Shallow)
//!    - WHY: Prevents runaway recursion in deeply nested directory structures
//!    - HOW: `recursive` flag controls `walkdir` max_depth (1 or usize::MAX) (line 156)
//!    - ATTACK PREVENTED: Infinite recursion via symlink loops or pathological structures
//!
//! 4. **Symlink Handling**
//!    - WHY: Prevents symlink loops and cross-mount escapes
//!    - HOW: `follow_links(false)` disables symlink following (line 158)
//!    - ATTACK PREVENTED: Symlink loops causing infinite traversal, escapes to `/etc`
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Read-only operation**: No mutation guard (all agents may list)
//! - **Glob filtering**: Enable filename-based filtering without regex complexity
//! - **Workspace-relative paths**: Return paths relative to mount root (not host filesystem)
//! - **Truncation transparency**: `truncated` flag indicates incomplete results
//! - **Fail-safe defaults**: 1000 result limit prevents memory exhaustion
//!
//! PERFORMANCE
//! ===========
//! - Uses `walkdir` streaming iterator (not materialized in memory)
//! - Glob matching is O(1) per filename after GlobSet compilation
//! - Result truncation at max_results prevents unbounded memory allocation
//! - Path stripping is O(1) prefix operation
//!
//! TRADE-OFFS
//! ==========
//! 1. **Truncation vs. Pagination**
//!    - CHOSEN: Truncate at max_results (not cursor-based pagination)
//!    - WHY: Simpler implementation, stateless operation
//!    - IMPLICATION: Callers cannot resume listing (must refine glob pattern)
//!
//! 2. **Symlink Following**
//!    - CHOSEN: Disable symlink following (follow_links: false)
//!    - WHY: Prevents symlink loops and cross-mount escapes
//!    - IMPLICATION: Symlinked directories are skipped (not traversed)
//!
//! 3. **Workspace-Relative vs. Absolute Paths**
//!    - CHOSEN: Return paths relative to workspace root (strip mount prefix)
//!    - WHY: Consistent with VFS abstraction (agents see virtual paths, not host paths)
//!    - IMPLICATION: Paths are portable across different host mount configurations

use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `fs:list` syscall.
///
/// WHY: Supports directory listing with optional glob filtering and recursion.
/// Result limiting prevents memory exhaustion from large directory trees.
#[derive(Debug, Deserialize)]
struct FsListArgs {
    /// Virtual path to directory (resolved via VFS). Defaults to ".".
    ///
    /// WHY: VFS resolution ensures path is within mounted workspace.
    #[serde(default)]
    path: String,

    /// Glob pattern for filename filtering (e.g., "*.rs", "test_*.txt").
    ///
    /// WHY: Enables filename-based filtering without regex complexity. Empty
    /// pattern matches all files.
    #[serde(default)]
    pattern: String,

    /// Enable recursive directory traversal.
    ///
    /// WHY: Controls walkdir max_depth (1 for shallow, usize::MAX for recursive).
    /// Default false to prevent accidental large traversals.
    #[serde(default)]
    recursive: bool,

    /// Maximum number of results to return.
    ///
    /// WHY: Prevents memory exhaustion from massive directory trees. Default 1000,
    /// capped at 5000 to limit Frame payload size.
    #[serde(default)]
    max_results: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for listing directory contents with VFS path validation.
///
/// WHY: Provides secure directory listing for agents while maintaining workspace
/// isolation through VFS resolution. Supports glob filtering and recursion.
pub struct FsList {
    /// VFS resolution strategy.
    ///
    /// WHY: Enables dependency injection for testing (Global, Disabled, or custom Table).
    vfs: VfsSource,
}

impl FsList {
    /// Create a new `FsList` syscall with global VFS.
    ///
    /// WHY: Standard constructor for production use with global mount table.
    pub fn new() -> Self {
        Self {
            vfs: VfsSource::Global,
        }
    }
}

impl Default for FsList {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsList {
    fn name(&self) -> &'static str {
        "fs:list"
    }

    /// List directory contents with VFS validation and glob filtering.
    ///
    /// WHY: Provides secure directory listing for agents while maintaining workspace
    /// isolation. Supports glob filtering to reduce result sets and recursion for
    /// deep directory traversal.
    ///
    /// USE CASE: Invoked by agents to discover files in workspace. Common patterns:
    /// - List current directory: `{ "path": "." }`
    /// - Find all Rust files: `{ "path": "src", "pattern": "*.rs", "recursive": true }`
    /// - List subdirectories only: `{ "path": ".", "recursive": false }`
    ///
    /// SECURITY NOTE: VFS resolution (line 134) is the trust boundary. Paths outside
    /// mounted workspaces are rejected before directory traversal occurs.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{ "matches": [...], "truncated": bool }` payload
    /// - `E_NOT_FOUND` if directory does not exist
    /// - `E_DISABLED` if VFS is not configured
    /// - `E_FORBIDDEN` if path is outside mounted workspace
    /// - `E_INVALID_ARGS` if glob pattern is malformed
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

        let args: FsListArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Cap max_results at 5000 to prevent Frame payload size explosion.
        // Default 1000 is reasonable for most directory listings.
        let max_results = args.max_results.unwrap_or(1000).min(5000);

        // ---------------------------------------------------------------------
        // PHASE 2: VFS Path Resolution (SECURITY BOUNDARY)
        // ---------------------------------------------------------------------
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

        // WHY: Default to "." (current directory) if path is empty or whitespace-only
        let path = if args.path.trim().is_empty() {
            ".".to_string()
        } else {
            args.path.clone()
        };

        // WHY: resolve() is the trust boundary - validates path against mount table
        let resolved = vfs.resolve(&path)?;
        let base = &resolved.host_path;

        if !base.exists() {
            return Err(KernelError::not_found(format!(
                "directory not found: {}",
                args.path
            )));
        }

        // ---------------------------------------------------------------------
        // PHASE 3: Glob Pattern Compilation
        // ---------------------------------------------------------------------
        // WHY: Compile glob pattern once before directory traversal (not per-file).
        // Empty pattern matches all files.
        let matcher: Option<GlobSet> = if !args.pattern.trim().is_empty() {
            let glob = Glob::new(args.pattern.trim())
                .map_err(|e| KernelError::invalid_args(format!("invalid pattern: {e}")))?;
            let mut builder = GlobSetBuilder::new();
            builder.add(glob);
            builder.build().ok()
        } else {
            None
        };

        // WHY: Compute workspace root for relative path stripping. Results are
        // returned as workspace-relative paths (not absolute host paths).
        let ws_root = &resolved.mount.host_path;

        // ---------------------------------------------------------------------
        // PHASE 4: Directory Traversal
        // ---------------------------------------------------------------------
        // WHY: Use walkdir with depth control and symlink protection.
        let mut out = Vec::new();
        let depth = if args.recursive { usize::MAX } else { 1 };
        for entry in walkdir::WalkDir::new(base)
            .follow_links(false)  // WHY: Prevent symlink loops and cross-mount escapes
            .max_depth(depth)      // WHY: Control recursion depth (1 or infinite)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            // WHY: Skip base directory itself (only return children)
            if entry.path() == base.as_path() {
                continue;
            }

            // WHY: Strip workspace root prefix to return virtual paths (portable across hosts)
            let rel = entry
                .path()
                .strip_prefix(ws_root)
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let Some(rel) = rel else { continue };

            // WHY: Apply glob filter to filename (not full path)
            if let Some(m) = &matcher {
                let name = entry.file_name().to_string_lossy();
                if !m.is_match(name.as_ref()) {
                    continue;
                }
            }

            out.push(rel);

            // WHY: Truncate at max_results to prevent memory exhaustion
            if out.len() >= max_results {
                break;
            }
        }

        // ---------------------------------------------------------------------
        // PHASE 5: Response Formatting
        // ---------------------------------------------------------------------
        // WHY: Sort results for deterministic output (easier to debug and test)
        out.sort();
        let truncated = out.len() >= max_results;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"matches": out, "truncated": truncated}),
            ))
            .await;

        Ok(())
    }
}
