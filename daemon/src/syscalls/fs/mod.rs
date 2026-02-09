//! Filesystem Operations - VFS-secured file and directory access
//!
//! All operations are mediated through a mount table that restricts access to
//! explicitly configured workspace directories. Unmatched paths resolve to an
//! in-memory filesystem (MemoryFs), providing agents scratch space without
//! exposing the host filesystem.

mod cd;
mod grep;
mod list;
mod mkdir;
mod read;
mod write;

pub use cd::FsCd;
pub use grep::FsGrep;
pub use list::FsList;
pub use mkdir::FsMkdir;
pub use read::FsRead;
pub use write::FsWrite;

use crate::kernel::KernelError;
use crate::vfs::{MountTable, VfsResolution, resolve_vfs_path};
use std::sync::Arc;

// =============================================================================
// VFS ABSTRACTION
// =============================================================================

/// VFS source abstraction for filesystem syscalls.
///
/// Every fs/ syscall stores a VfsSource and resolves it to a VfsResolution
/// (Host or Memory) before performing filesystem operations.
#[derive(Clone)]
pub(crate) enum VfsSource {
    /// Use global MountTable singleton (production default).
    Global,

    /// Filesystem access disabled (no VFS configured).
    Disabled,

    /// Use injected MountTable (for testing).
    Table(Arc<MountTable>),
}

impl VfsSource {
    /// Resolve a VFS path to either a host path or memory path.
    fn resolve(&self, path: &str) -> Result<VfsResolution, KernelError> {
        match self {
            VfsSource::Disabled => Err(KernelError::disabled(
                "filesystem access disabled: no mounts configured",
            )),
            VfsSource::Global => Ok(MountTable::global().resolve(path)?),
            VfsSource::Table(t) => Ok(t.resolve(path)?),
        }
    }

    /// Resolve a VFS path against the given CWD, then delegate to `resolve()`.
    /// Handles relative paths like `docs/foo.md` and `.` by joining with CWD first.
    fn resolve_with_cwd(&self, path: &str, vfs_cwd: &str) -> Result<VfsResolution, KernelError> {
        let absolute = resolve_vfs_path(path, vfs_cwd)?;
        self.resolve(&absolute)
    }

    /// Get a reference to the underlying MountTable (for mount_prefixes etc.).
    fn table(&self) -> Result<&MountTable, KernelError> {
        match self {
            VfsSource::Disabled => Err(KernelError::disabled(
                "filesystem access disabled: no mounts configured",
            )),
            VfsSource::Global => Ok(MountTable::global()),
            VfsSource::Table(t) => Ok(t.as_ref()),
        }
    }
}
