//! Fs:Read - Read file content with VFS path validation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides secure file reading within VFS-mounted workspaces. It implements
//! optional line slicing (offset/limit) for efficient partial file access without loading
//! entire files into memory when only a subset of lines is needed.
//!
//! **Integration points:**
//! - `HalFs` for platform-specific file I/O (enables testing with mock filesystems)
//! - `VfsSource` for VFS resolution (Global singleton, Disabled, or injected Table)
//! - `SyscallContext` for cancellation propagation
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{ "content": "..." }` payload
//! - Returns `E_NOT_FOUND` for missing files, `E_DISABLED` for VFS unavailable, `E_IO` for read errors
//!
//! SECURITY MODEL
//! ==============
//! 1. **VFS Path Resolution** (Primary Security Boundary)
//!    - WHY: Prevents reading files outside mounted workspaces
//!    - HOW: `vfs.resolve(&args.path)` validates path against mount table (line 132)
//!    - ATTACK PREVENTED: Path traversal (`../../etc/passwd`), absolute paths (`/etc/shadow`)
//!
//! 2. **Read-only Operation** (No Mutation Guard)
//!    - WHY: Reading files does not modify state, so all agents may read
//!    - HOW: No `ctx.require_mutation()` check (unlike fs:write)
//!    - RATIONALE: "Hand" agents (controlled by LLMs) need file access for code understanding
//!
//! 3. **UTF-8 Lossy Conversion**
//!    - WHY: Prevents crashes on binary files or invalid UTF-8 sequences
//!    - HOW: `read_to_string()` uses lossy conversion (replaces invalid bytes with �)
//!    - TRADE-OFF: Binary file content may be corrupted, but operation succeeds
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Partial reads via line slicing**: Efficient access to log tails or large file snippets
//! - **UTF-8 first**: Assume text files (source code, configs, docs) as primary use case
//! - **Fail-fast validation**: VFS resolution fails before file I/O on invalid paths
//! - **Error clarity**: Distinguish file-not-found from I/O errors for better debugging
//!
//! PERFORMANCE
//! ===========
//! - Reads entire file into memory, then slices lines (not streaming)
//! - TRADE-OFF: Simplicity over memory efficiency (acceptable for typical source files <1MB)
//! - Line slicing avoids sending full file content in Frame response (reduces IPC cost)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Full Read vs. Streaming**
//!    - CHOSEN: Read entire file, then slice lines in memory
//!    - WHY: Simpler implementation, typical files fit in memory
//!    - COST: Large files (>10MB) consume memory even if only 100 lines needed
//!    - ACCEPTABLE: Most source files are <1MB, workspace isolation limits risk
//!
//! 2. **Line-Based Slicing vs. Byte Ranges**
//!    - CHOSEN: Offset/limit operate on line counts (not byte offsets)
//!    - WHY: More intuitive for text files (agents think in lines, not bytes)
//!    - IMPLICATION: Cannot seek to byte offsets (e.g., resume partial read)

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalFs, HostHalFs};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `fs:read` syscall.
///
/// WHY: Supports both full file reads and partial line slicing. Offset/limit
/// enable efficient access to log tails or large file snippets without loading
/// entire file into Frame response.
#[derive(Debug, Deserialize)]
struct FsReadArgs {
    /// Virtual path to file (resolved via VFS).
    ///
    /// WHY: VFS resolution ensures path is within mounted workspace.
    path: String,

    /// Maximum number of lines to return.
    ///
    /// WHY: Limits Frame response size for large files. Typical use case:
    /// "Show me the first 100 lines of server.log".
    #[serde(default)]
    limit: Option<usize>,

    /// Line offset to start reading from (0-indexed).
    ///
    /// WHY: Enables pagination or tail-like access. Typical use case:
    /// "Show me lines 1000-1100 of server.log" (offset=1000, limit=100).
    #[serde(default)]
    offset: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for reading file content with VFS path validation.
///
/// WHY: Provides secure file access for agents while maintaining workspace
/// isolation through VFS resolution. Supports line slicing for efficient
/// partial file access.
pub struct FsRead {
    /// Hardware abstraction for filesystem operations.
    ///
    /// WHY: Enables testing with mock filesystems that simulate I/O failures,
    /// missing files, or invalid UTF-8 content.
    fs: Arc<dyn HalFs>,

    /// VFS resolution strategy.
    ///
    /// WHY: Enables dependency injection for testing (Global, Disabled, or custom Table).
    vfs: VfsSource,
}

impl FsRead {
    /// Create a new `FsRead` syscall with global VFS.
    ///
    /// WHY: Standard constructor for production use with global mount table.
    pub fn new() -> Self {
        Self {
            fs: Arc::new(HostHalFs),
            vfs: VfsSource::Global,
        }
    }

    /// Create a `FsRead` syscall with filesystem access disabled.
    ///
    /// WHY: Useful for agents that should not have workspace access (e.g., pure network agents).
    pub fn disabled() -> Self {
        Self {
            fs: Arc::new(HostHalFs),
            vfs: VfsSource::Disabled,
        }
    }

    /// Create a `FsRead` syscall with custom HAL filesystem.
    ///
    /// WHY: Enables testing with mock filesystems that simulate failures or custom behavior.
    pub fn with_fs(fs: Arc<dyn HalFs>) -> Self {
        Self {
            fs,
            vfs: VfsSource::Global,
        }
    }

    /// Create a `FsRead` syscall with custom VFS mount table.
    ///
    /// WHY: Enables testing with temporary directories and custom mount configurations.
    #[allow(dead_code)]
    pub fn with_vfs(fs: Arc<dyn HalFs>, vfs: Arc<MountTable>) -> Self {
        Self {
            fs,
            vfs: VfsSource::Table(vfs),
        }
    }
}

impl Default for FsRead {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsRead {
    fn name(&self) -> &'static str {
        "fs:read"
    }

    /// Read file content with VFS path validation and optional line slicing.
    ///
    /// WHY: Provides secure file reading for agents while maintaining workspace isolation.
    /// Supports line slicing for efficient access to large files or log tails.
    ///
    /// USE CASE: Invoked by agents to read source files, configs, or logs. Common patterns:
    /// - Read entire file: `{ "path": "src/main.rs" }`
    /// - Read first 100 lines: `{ "path": "server.log", "limit": 100 }`
    /// - Read log tail: `{ "path": "server.log", "offset": 9900, "limit": 100 }`
    ///
    /// SECURITY NOTE: VFS resolution (line 132) is the trust boundary. Paths outside
    /// mounted workspaces are rejected before file I/O occurs.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{ "content": "..." }` payload (UTF-8 string)
    /// - `E_NOT_FOUND` if file does not exist
    /// - `E_DISABLED` if VFS is not configured
    /// - `E_FORBIDDEN` if path is outside mounted workspace
    /// - `E_IO` for filesystem errors (permission denied, I/O error, etc.)
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

        let args: FsReadArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.path.is_empty() {
            return Err(KernelError::invalid_args(
                "'path' is required and cannot be empty",
            ));
        }

        // ---------------------------------------------------------------------
        // PHASE 2: VFS Path Resolution (SECURITY BOUNDARY)
        // ---------------------------------------------------------------------
        // WHY: VFS resolution validates path is within mounted workspace before
        // file I/O occurs. This prevents path traversal and absolute path escapes.
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

        // WHY: resolve() is the trust boundary - validates path against mount table
        let resolved = vfs.resolve(&args.path)?;

        ctx.check_cancelled()?;

        // ---------------------------------------------------------------------
        // PHASE 3: File Reading
        // ---------------------------------------------------------------------
        // WHY: read_to_string() uses lossy UTF-8 conversion (replaces invalid bytes with �).
        // Prevents crashes on binary files, but may corrupt binary content.
        let content = self
            .fs
            .read_to_string(&resolved.host_path)
            .await
            .map_err(|e| {
                // WHY: Distinguish file-not-found from I/O errors for clearer debugging
                if e.to_string().contains("No such file") || e.to_string().contains("not found") {
                    KernelError::not_found(format!("file not found: {}", args.path))
                } else {
                    KernelError::io(e.to_string())
                }
            })?;

        // ---------------------------------------------------------------------
        // PHASE 4: Line Slicing (Optional)
        // ---------------------------------------------------------------------
        // WHY: Slice lines after full read (not streaming). Simpler implementation,
        // acceptable memory cost for typical source files (<1MB).
        let result = if args.offset.is_some() || args.limit.is_some() {
            let lines: Vec<&str> = content.lines().collect();
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit.unwrap_or(lines.len());

            // WHY: skip(offset).take(limit) provides pagination semantics
            let selected: Vec<&str> = lines.into_iter().skip(offset).take(limit).collect();
            selected.join("\n")
        } else {
            content
        };

        // ---------------------------------------------------------------------
        // PHASE 5: Response Formatting
        // ---------------------------------------------------------------------
        tx.send(Frame::ok(ctx.call_id, json!({ "content": result })))
            .await
            .ok();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn make_ctx(cwd: &std::path::Path) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
    }

    #[tokio::test]
    async fn test_fs_read_no_vfs_returns_disabled() {
        let syscall = FsRead::disabled();
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(&ctx, json!({ "path": "/test.txt" }), tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_DISABLED");
    }

    #[tokio::test]
    async fn test_fs_read_empty_path_rejected() {
        let syscall = FsRead::new();
        let ctx = make_ctx(std::path::Path::new("/tmp"));
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall.execute(&ctx, json!({ "path": "" }), tx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
    }
}
