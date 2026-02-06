//! Fs:Mkdir - Create directories with VFS path validation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides secure directory creation within VFS-mounted workspaces. It implements
//! **two-tier security** through VFS path validation and actor authorization. Optional parent
//! directory creation (`parents` flag) enables recursive directory creation.
//!
//! **Integration points:**
//! - `HalFs` for platform-specific directory creation (create_dir vs. create_dir_all)
//! - `VfsSource` for VFS resolution (validates path within mounts)
//! - `SyscallContext` for actor verification and cancellation
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{ "created": bool, "path": "..." }` payload
//! - `created: false` if directory already exists (idempotent behavior)
//! - Returns `E_FORBIDDEN` for actor violations, `E_IO` for filesystem errors
//!
//! SECURITY MODEL
//! ==============
//! 1. **VFS Path Resolution** (Primary Security Boundary)
//!    - WHY: Prevents creating directories outside mounted workspaces
//!    - HOW: `vfs.resolve(&args.path)` validates path against mount table (line 119)
//!    - ATTACK PREVENTED: Path traversal (`../../tmp/backdoor`), absolute paths (`/tmp/malicious`)
//!
//! 2. **Actor Authorization** (Mutation Permission)
//!    - WHY: Only "head" agents may create directories (prevents LLM-controlled agents from filesystem pollution)
//!    - HOW: `ctx.require_mutation()` enforces "head" actor requirement (line 100)
//!    - ATTACK PREVENTED: Compromised "hand" agents cannot create directories for privilege escalation
//!    - TIMING: Checked early (before VFS resolution) to fail fast on unauthorized requests
//!
//! 3. **Idempotent Behavior**
//!    - WHY: Prevents errors when directory already exists (common in scripts)
//!    - HOW: Check `exists()` before creation, return `created: false` if present (line 125)
//!    - RATIONALE: Aligns with POSIX `mkdir -p` semantics (success if directory exists)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Defense-in-depth**: Actor authorization → VFS resolution → Filesystem operation
//! - **Idempotent operations**: Success if directory exists (not an error)
//! - **Parent creation default**: `parents: true` by default (convenience over strictness)
//! - **Workspace-relative paths**: Return virtual paths (not absolute host paths)
//!
//! PERFORMANCE
//! ===========
//! - Existence check via `exists()` syscall (single I/O operation)
//! - Parent creation uses recursive `create_dir_all()` (OS-optimized)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Parents Flag Default**
//!    - CHOSEN: `parents: true` by default (opposite of fs:write's `create_dirs: false`)
//!    - WHY: Directory creation typically needs parents (e.g., `mkdir src/new/module`)
//!    - RATIONALE: Aligns with `mkdir -p` convention (most common use case)
//!
//! 2. **Idempotent vs. Error on Exists**
//!    - CHOSEN: Return success if directory exists (not E_CONFLICT)
//!    - WHY: Simpler scripting (no need to check existence before mkdir)
//!    - IMPLICATION: Callers cannot distinguish "created" from "already existed" (check response flag)

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

/// Arguments for `fs:mkdir` syscall.
///
/// WHY: Supports both single-level and recursive directory creation via `parents` flag.
#[derive(Debug, Deserialize)]
struct FsMkdirArgs {
    /// Virtual path to directory (resolved via VFS).
    ///
    /// WHY: VFS resolution ensures path is within mounted workspace.
    path: String,

    /// Create parent directories if they don't exist.
    ///
    /// WHY: Enables recursive directory creation (like `mkdir -p`). Default true
    /// for convenience (most use cases need parents).
    #[serde(default)]
    parents: Option<bool>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for creating directories with VFS path validation.
///
/// WHY: Provides secure directory creation for agents while maintaining workspace
/// isolation through VFS resolution. Supports recursive parent creation.
pub struct FsMkdir {
    /// VFS resolution strategy.
    ///
    /// WHY: Enables dependency injection for testing (Global, Disabled, or custom Table).
    vfs: VfsSource,
}

impl FsMkdir {
    /// Create a new `FsMkdir` syscall with global VFS.
    ///
    /// WHY: Standard constructor for production use with global mount table.
    pub fn new() -> Self {
        Self {
            vfs: VfsSource::Global,
        }
    }

    /// Create a new `FsMkdir` syscall with an injected VFS mount table.
    #[allow(dead_code)]
    pub fn with_vfs(vfs: Arc<MountTable>) -> Self {
        Self {
            vfs: VfsSource::Table(vfs),
        }
    }
}

impl Default for FsMkdir {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsMkdir {
    fn name(&self) -> &'static str {
        "fs:mkdir"
    }

    /// Create directory with VFS and actor validation.
    ///
    /// WHY: Provides secure directory creation for agents while enforcing workspace
    /// isolation and actor authorization.
    ///
    /// USE CASE: Invoked by "head" agents to create directory structures for generated
    /// code, test fixtures, or build artifacts. Common patterns:
    /// - Create single directory: `{ "path": "build", "parents": false }`
    /// - Create nested structure: `{ "path": "src/new/module" }` (parents: true by default)
    ///
    /// SECURITY NOTE: Two-tier validation:
    /// 1. Actor authorization (line 100) - only "head" agents may create directories
    /// 2. VFS resolution (line 119) - path must be within mounted workspace
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{ "created": true, "path": "..." }` if directory created
    /// - `Frame::ok` with `{ "created": false, "path": "..." }` if directory already exists (idempotent)
    /// - `E_FORBIDDEN` if actor lacks mutation permission or path outside workspace
    /// - `E_DISABLED` if VFS is not configured
    /// - `E_INVALID_ARGS` if path is empty
    /// - `E_IO` for filesystem errors (permission denied, read-only filesystem, etc.)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // ---------------------------------------------------------------------
        // PHASE 1: Security Verification
        // ---------------------------------------------------------------------
        // WHY: Check cancellation and actor authorization before VFS resolution
        ctx.check_cancelled()?;

        // WHY: Only "head" agents may create directories
        ctx.require_mutation()?;

        // ---------------------------------------------------------------------
        // PHASE 2: Argument Parsing & Validation
        // ---------------------------------------------------------------------
        let args: FsMkdirArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        if args.path.trim().is_empty() {
            return Err(KernelError::invalid_args("path is required"));
        }

        // ---------------------------------------------------------------------
        // PHASE 3: VFS Path Resolution (SECURITY BOUNDARY)
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

        // WHY: resolve() is the trust boundary - validates path against mount table
        let resolved = vfs.resolve(&args.path)?;
        let full = &resolved.host_path;
        let ws_root = &resolved.mount.host_path;

        // WHY: Default parents to true for convenience (aligns with mkdir -p)
        let parents = args.parents.unwrap_or(true);

        // ---------------------------------------------------------------------
        // PHASE 4: Idempotent Existence Check
        // ---------------------------------------------------------------------
        // WHY: Check if directory already exists before attempting creation.
        // Return success if exists (idempotent behavior like mkdir -p).
        let exists = HostHalFs.exists(full).await.unwrap_or(false);
        if exists {
            let rel = full
                .strip_prefix(ws_root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| args.path.clone());
            let _ = tx
                .send(Frame::ok(
                    ctx.call_id,
                    json!({"created": false, "path": rel}),
                ))
                .await;
            return Ok(());
        }

        // ---------------------------------------------------------------------
        // PHASE 5: Directory Creation
        // ---------------------------------------------------------------------
        // WHY: Use create_dir_all() if parents=true (recursive), create_dir() otherwise (single-level)
        let res = if parents {
            HostHalFs.create_dir_all(full).await
        } else {
            HostHalFs.create_dir(full).await
        };

        // ---------------------------------------------------------------------
        // PHASE 6: Response Formatting
        // ---------------------------------------------------------------------
        match res {
            Ok(_) => {
                // WHY: Strip workspace root prefix to return virtual path
                let rel = full
                    .strip_prefix(ws_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| args.path.clone());
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({"created": true, "path": rel}),
                    ))
                    .await;
                Ok(())
            }
            Err(e) => Err(KernelError::io(format!("mkdir error: {e}"))),
        }
    }
}
