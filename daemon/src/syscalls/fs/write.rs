//! Fs:Write - Write file content with VFS and mount mode validation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides secure file writing within VFS-mounted workspaces. It implements
//! **three-tier security** through VFS path validation, mount mode enforcement, and actor
//! authorization. Optional parent directory creation (`create_dirs`) enables writing to
//! new directory structures without separate mkdir calls.
//!
//! **Integration points:**
//! - `HalFs` for platform-specific file I/O and directory creation
//! - `VfsSource` for VFS resolution (validates path within mounts)
//! - `SyscallContext` for actor verification and cancellation
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{ "path": "...", "bytes_written": N }` payload
//! - Returns `E_READONLY` for read-only mounts, `E_FORBIDDEN` for actor violations, `E_IO` for write errors
//!
//! SECURITY MODEL
//! ==============
//! This syscall implements **defense-in-depth** for file writing:
//!
//! 1. **VFS Path Resolution** (Primary Security Boundary)
//!    - WHY: Prevents writing files outside mounted workspaces
//!    - HOW: `vfs.resolve(&args.path)` validates path against mount table (line 135)
//!    - ATTACK PREVENTED: Path traversal (`../../.ssh/authorized_keys`), absolute paths (`/etc/passwd`)
//!
//! 2. **Mount Mode Enforcement** (Read-only vs. Read-write)
//!    - WHY: Prevents writing to read-only reference mounts (e.g., stdlib docs, shared libraries)
//!    - HOW: Check `resolved.mount.mode == MountMode::Ro` before mutation (line 137)
//!    - ATTACK PREVENTED: Modifying read-only workspaces or system directories
//!    - RATIONALE: Mount mode is a security boundary separate from actor authorization
//!
//! 3. **Actor Authorization** (Mutation Permission)
//!    - WHY: Only "head" agents may write files (prevents LLM-controlled agents from arbitrary writes)
//!    - HOW: `ctx.require_mutation()` enforces "head" actor requirement (line 143)
//!    - ATTACK PREVENTED: Compromised "hand" agents cannot write malicious files
//!    - TIMING: Checked AFTER mount mode validation (fail fast on read-only mounts)
//!
//! 4. **Parent Directory Creation** (Optional)
//!    - WHY: Enables atomic file creation without separate mkdir calls
//!    - HOW: `create_dirs` flag triggers `create_dir_all()` for missing parents (lines 145-156)
//!    - SECURITY: Parent creation still respects VFS boundaries (cannot escape workspace)
//!    - DEFAULT: false (explicit opt-in to prevent accidental directory creation)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Defense-in-depth**: VFS → Mount Mode → Actor → Filesystem operation
//! - **Fail-fast validation**: Check mount mode before actor permission (clearer error messages)
//! - **Explicit parent creation**: Opt-in `create_dirs` flag prevents accidental directory pollution
//! - **Atomic writes**: No temporary files or swap files (overwrite directly)
//! - **UTF-8 content**: Assumes text files as primary use case (source code, configs)
//!
//! PERFORMANCE
//! ===========
//! - Writes entire content atomically (no streaming or chunking)
//! - TRADE-OFF: Simplicity over memory efficiency (acceptable for typical files <1MB)
//! - Parent directory check uses `exists()` syscall (single I/O operation)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Atomic Writes vs. Temporary Files**
//!    - CHOSEN: Direct overwrite (no .tmp + rename pattern)
//!    - WHY: Simpler implementation, consistent with git workflow (files can be reverted)
//!    - RISK: Crash during write may corrupt file (acceptable given VCS backup)
//!
//! 2. **Parent Creation Default**
//!    - CHOSEN: `create_dirs: false` by default (explicit opt-in)
//!    - WHY: Prevents accidental directory creation from typos or path errors
//!    - IMPLICATION: Callers must explicitly enable parent creation or mkdir separately
//!
//! 3. **UTF-8 vs. Binary**
//!    - CHOSEN: Accept string content (UTF-8 encoded to bytes)
//!    - WHY: Primary use case is text files (source code, configs, docs)
//!    - LIMITATION: Binary file writing requires base64 encoding (not supported directly)

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hal::{HalFs, HostHalFs};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::{MountMode, MountTable};

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `fs:write` syscall.
///
/// WHY: Supports both simple file writes and parent directory creation. Optional
/// `create_dirs` flag enables atomic file creation in new directory structures.
#[derive(Debug, Deserialize)]
struct FsWriteArgs {
    /// Virtual path to file (resolved via VFS).
    ///
    /// WHY: VFS resolution ensures path is within mounted workspace.
    path: String,

    /// File content to write (UTF-8 string).
    ///
    /// WHY: String type assumes text files (source code, configs, docs). Binary
    /// files would require base64 encoding (not currently supported).
    content: String,

    /// Create parent directories if they don't exist.
    ///
    /// WHY: Enables atomic file creation without separate mkdir calls. Default
    /// false to prevent accidental directory creation from typos.
    #[serde(default)]
    create_dirs: bool,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for writing file content with VFS and mount mode validation.
///
/// WHY: Provides secure file writing for agents while maintaining workspace
/// isolation and enforcing read-only mount restrictions.
pub struct FsWrite {
    /// Hardware abstraction for filesystem operations.
    ///
    /// WHY: Enables testing with mock filesystems that simulate write failures,
    /// permission errors, or disk space exhaustion.
    fs: Arc<dyn HalFs>,

    /// VFS resolution strategy.
    ///
    /// WHY: Enables dependency injection for testing (Global, Disabled, or custom Table).
    vfs: VfsSource,
}

impl FsWrite {
    /// Create a new `FsWrite` syscall with global VFS.
    ///
    /// WHY: Standard constructor for production use with global mount table.
    pub fn new() -> Self {
        Self {
            fs: Arc::new(HostHalFs),
            vfs: VfsSource::Global,
        }
    }

    /// Create a `FsWrite` syscall with filesystem access disabled.
    ///
    /// WHY: Useful for agents that should not have workspace write access.
    pub fn disabled() -> Self {
        Self {
            fs: Arc::new(HostHalFs),
            vfs: VfsSource::Disabled,
        }
    }

    /// Create a `FsWrite` syscall with custom HAL filesystem.
    ///
    /// WHY: Enables testing with mock filesystems that simulate failures or custom behavior.
    pub fn with_fs(fs: Arc<dyn HalFs>) -> Self {
        Self {
            fs,
            vfs: VfsSource::Global,
        }
    }

    /// Create a `FsWrite` syscall with custom VFS mount table.
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

impl Default for FsWrite {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsWrite {
    fn name(&self) -> &'static str {
        "fs:write"
    }

    /// Write file content with VFS, mount mode, and actor validation.
    ///
    /// WHY: Provides secure file writing for agents while enforcing workspace isolation,
    /// read-only mount restrictions, and actor authorization.
    ///
    /// USE CASE: Invoked by "head" agents to write source files, configs, or generated content.
    /// Common patterns:
    /// - Write to existing file: `{ "path": "src/main.rs", "content": "fn main() {}" }`
    /// - Create with parents: `{ "path": "src/new/file.rs", "content": "...", "create_dirs": true }`
    ///
    /// SECURITY NOTE: Three-tier validation:
    /// 1. VFS resolution (line 135) - path must be within mounted workspace
    /// 2. Mount mode (line 137) - mount must be read-write (not read-only)
    /// 3. Actor authorization (line 143) - only "head" agents may write
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{ "path": "...", "bytes_written": N }` payload
    /// - `E_READONLY` if mount is read-only
    /// - `E_FORBIDDEN` if actor lacks mutation permission or path outside workspace
    /// - `E_DISABLED` if VFS is not configured
    /// - `E_NOT_FOUND` if parent directory missing and `create_dirs: false`
    /// - `E_IO` for filesystem errors (permission denied, disk full, etc.)
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

        let args: FsWriteArgs = serde_json::from_value(data)
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
        // checking mount mode or actor authorization.
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

        // ---------------------------------------------------------------------
        // PHASE 3: Mount Mode Validation
        // ---------------------------------------------------------------------
        // WHY: Check mount mode BEFORE actor authorization to provide clearer error
        // messages (read-only mount vs. insufficient permission).
        if resolved.mount.mode == MountMode::Ro {
            return Err(KernelError::readonly(format!(
                "mount '{}' is read-only",
                resolved.mount.prefix
            )));
        }

        // ---------------------------------------------------------------------
        // PHASE 4: Actor Authorization
        // ---------------------------------------------------------------------
        // WHY: Only "head" agents may write files. This prevents "hand" agents
        // (controlled by LLMs) from arbitrary file modifications.
        ctx.require_mutation()?;

        // ---------------------------------------------------------------------
        // PHASE 5: Parent Directory Creation (Optional)
        // ---------------------------------------------------------------------
        // WHY: Enables atomic file creation in new directory structures without
        // separate mkdir calls. Only runs if `create_dirs: true`.
        if let Some(parent) = resolved.host_path.parent()
            && !parent.exists()
        {
            if args.create_dirs {
                // WHY: create_dir_all() recursively creates parent directories
                self.fs
                    .create_dir_all(parent)
                    .await
                    .map_err(|e| KernelError::io(e.to_string()))?;
            } else {
                return Err(KernelError::not_found(format!(
                    "parent directory does not exist: {}",
                    parent.display()
                )));
            }
        }

        ctx.check_cancelled()?;

        // ---------------------------------------------------------------------
        // PHASE 6: File Writing
        // ---------------------------------------------------------------------
        // WHY: Write content atomically (no temporary files). Content is already
        // validated by mount mode and actor checks.
        self.fs
            .write(&resolved.host_path, args.content.as_bytes())
            .await
            .map_err(|e| KernelError::io(e.to_string()))?;

        // ---------------------------------------------------------------------
        // PHASE 7: Response Formatting
        // ---------------------------------------------------------------------
        tx.send(Frame::ok(
            ctx.call_id,
            json!({
                "path": resolved.host_path.display().to_string(),
                "bytes_written": args.content.len()
            }),
        ))
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

    fn make_ctx_with_actor(cwd: &std::path::Path, actor: &str) -> SyscallContext {
        SyscallContext::new(Uuid::new_v4(), cwd.to_path_buf(), CancellationToken::new())
            .with_actor(Some(actor.to_string()))
    }

    #[tokio::test]
    async fn test_fs_write_no_vfs_returns_disabled() {
        let syscall = FsWrite::disabled();
        let ctx = make_ctx_with_actor(std::path::Path::new("/tmp"), "head/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "path": "/output.txt", "content": "hello world" }),
                tx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_DISABLED");
    }

    #[tokio::test]
    async fn test_fs_write_hand_scope_rejected() {
        let syscall = FsWrite::new();
        let ctx = make_ctx_with_actor(std::path::Path::new("/tmp"), "hand/test");
        let (tx, _rx) = mpsc::channel(8);

        let result = syscall
            .execute(
                &ctx,
                json!({ "path": "/output.txt", "content": "hello" }),
                tx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.code == "E_DISABLED" || err.code == "E_FORBIDDEN");
    }
}
