//! Filesystem Operations - VFS-secured file and directory access
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides filesystem operations (read, write, list, search, mkdir, diff)
//! with **Virtual Filesystem (VFS) security boundaries**. All operations are mediated through
//! a mount table that restricts access to explicitly configured workspace directories,
//! preventing filesystem escapes via path traversal or absolute paths.
//!
//! **VFS Architecture:**
//! - `MountTable::global()` - Global singleton for production use
//! - `VfsSource` - Abstraction enabling VFS injection for testing
//! - Path resolution via `vfs.resolve()` - Trust boundary for all operations
//! - Mount modes (Read-only vs. Read-write) enforced per-mount
//!
//! **Registered syscalls:**
//! - `fs:read` - Read file content with optional line slicing
//! - `fs:write` - Write file content (requires mutation permission)
//! - `fs:list` - List directory contents with glob filtering
//! - `fs:search` - Content search with regex/literal matching
//! - `fs:mkdir` - Create directories with parent creation option
//! - `fs:diff` - Generate unified diffs between files
//!
//! **Integration points:**
//! - `HalFs` for platform-specific filesystem operations
//! - `MountTable` for VFS path resolution and mount configuration
//! - `SyscallContext` for cancellation, actor verification, and working directory
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with operation-specific result payloads
//! - Returns `KernelError` for VFS violations, permission errors, or I/O failures
//!
//! SECURITY MODEL
//! ==============
//! This namespace implements **defense-in-depth** through VFS isolation:
//!
//! 1. **VFS Path Resolution** (Primary Security Boundary)
//!    - WHY: Prevents filesystem escapes outside mounted workspaces
//!    - HOW: Every operation calls `vfs.resolve()` before filesystem access
//!    - ATTACK PREVENTED: Path traversal via `../../etc/passwd`, absolute paths like `/etc/shadow`
//!    - IMPLEMENTATION: VFS validates paths against mount table and normalizes to host paths
//!
//! 2. **Mount Isolation**
//!    - WHY: Limits filesystem access to explicitly configured directories
//!    - HOW: Mountpoints defined at kernel initialization (e.g., `/workspace` → `/Users/foo/project`)
//!    - ATTACK PREVENTED: Agents cannot access files outside workspace (home dir, system files)
//!    - TRADE-OFF: Requires upfront mount configuration (not dynamic)
//!
//! 3. **Mount Mode Enforcement** (Read-only vs. Read-write)
//!    - WHY: Restricts write operations to writable mounts
//!    - HOW: `fs:write` checks `resolved.mount.mode == MountMode::Ro` before mutation
//!    - ATTACK PREVENTED: Writing to read-only reference mounts (e.g., stdlib docs)
//!    - IMPLEMENTATION: Each mount has a mode flag (Ro/Rw) checked at syscall execution
//!
//! 4. **Actor Authorization** (Mutation Operations)
//!    - WHY: Only "head" agents may modify filesystem state
//!    - HOW: `ctx.require_mutation()` enforces actor permission for write/mkdir
//!    - ATTACK PREVENTED: Compromised "hand" agents (controlled by LLMs) cannot write files
//!    - IMPLEMENTATION: Mutation guard checked after VFS resolution, before filesystem operation
//!
//! 5. **Output Limiting** (Search/List Operations)
//!    - WHY: Prevents memory exhaustion from large result sets
//!    - HOW: `max_results` parameter (default 200-1000, cap 5000) truncates output
//!    - ATTACK PREVENTED: Resource exhaustion DoS from recursive directory traversal
//!    - TRADE-OFF: Large result sets require multiple queries with pagination
//!
//! 6. **Cancellation Propagation**
//!    - WHY: Ensures long-running operations (search, diff) terminate when task cancels
//!    - HOW: `ctx.check_cancelled()` checked before expensive operations
//!    - ATTACK PREVENTED: Orphaned filesystem operations consuming resources
//!    - IMPLEMENTATION: Cancellation token passed to HAL layer for process-based operations
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **VFS-first security**: All filesystem access mediated through VFS resolution
//! - **Fail-safe defaults**: VFS disabled by default (explicit mount configuration required)
//! - **Explicit authorization**: Mutation operations require actor permission AND writable mount
//! - **Path normalization**: VFS resolves relative paths, strips traversal attempts
//! - **Mount isolation**: Workspaces are security boundaries (cannot cross mounts)
//! - **Binary-safe operations**: UTF-8 lossy conversion for content (no binary corruption)
//!
//! VFS RESOLUTION PATTERN
//! ======================
//! Every filesystem syscall follows this pattern:
//!
//! ```rust
//! // 1. Get VFS instance (Global, Disabled, or injected Table)
//! let vfs = match &self.vfs {
//!     VfsSource::Disabled => return Err(KernelError::disabled("...")),
//!     VfsSource::Global => MountTable::global().ok_or(...)?,
//!     VfsSource::Table(t) => t.as_ref(),
//! };
//!
//! // 2. Resolve virtual path to host path (TRUST BOUNDARY)
//! let resolved = vfs.resolve(&args.path)?;
//! let host_path = resolved.host_path;  // Safe host filesystem path
//! let mount = resolved.mount;           // Mount configuration (mode, prefix)
//!
//! // 3. Perform filesystem operation on resolved host path
//! self.fs.read_to_string(&host_path).await?;
//! ```
//!
//! WHY THIS PATTERN:
//! - **Trust boundary**: VFS resolve() is the single point where path validation occurs
//! - **Fail-fast**: VFS resolution fails immediately on invalid paths (before I/O)
//! - **Consistency**: All syscalls use identical pattern (auditable, testable)
//! - **Testability**: VfsSource enables mock VFS injection for unit tests
//!
//! PERFORMANCE
//! ===========
//! - VFS resolution is O(log n) in number of mounts (typically 1-5 mounts)
//! - Search operations are bounded by `max_results` to prevent runaway queries
//! - Diff operations truncate output at 20KB to limit Frame payload size
//! - List operations use `walkdir` streaming (not materialized in memory)
//!
//! CONCURRENCY
//! ===========
//! - All operations are async and yield during I/O (non-blocking)
//! - VFS `MountTable` is Arc-wrapped for shared access across tasks
//! - Mutation operations serialize via actor model (only one "head" agent active)
//! - Read operations are safe for concurrent execution
//!
//! TRADE-OFFS
//! ==========
//! 1. **VFS Overhead vs. Security**
//!    - CHOSEN: Mandatory VFS resolution for all operations
//!    - COST: Extra path validation step (map lookup, prefix strip)
//!    - WHY: Security boundary prevents 99% of filesystem attacks
//!    - ACCEPTABLE: Path resolution is <1% of I/O operation cost
//!
//! 2. **Mount Configuration vs. Dynamic Access**
//!    - CHOSEN: Static mount configuration at kernel initialization
//!    - REJECTED: Dynamic mount addition/removal during runtime
//!    - WHY: Prevents privilege escalation via mount manipulation
//!    - IMPLICATION: Workspaces must be known upfront (not discovered)
//!
//! 3. **Mount Isolation vs. Symlink Following**
//!    - CHOSEN: VFS resolves symlinks within mounts, rejects cross-mount symlinks
//!    - WHY: Symlinks could escape workspace boundaries
//!    - IMPLICATION: Symlinks to `/etc/passwd` fail even if they exist in workspace
//!    - IMPLEMENTATION: `canonicalize()` resolves symlinks before mount validation
//!
//! 4. **Binary vs. Text Operations**
//!    - CHOSEN: UTF-8 lossy conversion for all file content
//!    - WHY: Prevents crashes on binary/invalid UTF-8 content
//!    - IMPLICATION: Binary files may have corrupted content in responses
//!    - ACCEPTABLE: Agents primarily work with text files (source code, docs)
//!
//! 5. **Truncation vs. Streaming**
//!    - CHOSEN: Result truncation at fixed limits (5000 matches, 20KB diffs)
//!    - REJECTED: Streaming results via Frame::event protocol
//!    - WHY: Simpler implementation, bounded memory usage
//!    - IMPLICATION: Large result sets require multiple queries or pagination

mod diff;
mod list;
mod mkdir;
mod read;
mod search;
mod write;

pub use diff::FsDiff;
pub use list::FsList;
pub use mkdir::FsMkdir;
pub use read::FsRead;
pub use search::FsSearch;
pub use write::FsWrite;

use std::sync::Arc;
use crate::vfs::MountTable;

// =============================================================================
// VFS ABSTRACTION
// =============================================================================

/// VFS source abstraction for filesystem syscalls.
///
/// WHY: Enables dependency injection for testing with mock VFS mount tables.
/// Production use cases reference `Global` (MountTable singleton), while tests
/// inject `Table` with custom mount configurations. `Disabled` mode is used
/// when no filesystem access should be permitted (e.g., network-only agents).
///
/// PATTERN: Every fs/ syscall stores a VfsSource and resolves it to a MountTable
/// reference before path resolution. This indirection enables both production
/// and test scenarios without conditional compilation.
///
/// TRADE-OFF: Adds one extra enum match per syscall execution (negligible cost)
/// for significant testability improvement.
#[derive(Clone)]
pub(crate) enum VfsSource {
    /// Use global MountTable singleton (production default).
    ///
    /// WHY: Most syscalls use the kernel's global mount configuration.
    Global,

    /// Filesystem access disabled (no VFS configured).
    ///
    /// WHY: Explicitly disables filesystem operations for agents that should
    /// not have workspace access (e.g., pure network/computation agents).
    Disabled,

    /// Use injected MountTable (for testing).
    ///
    /// WHY: Tests inject custom mount tables with temporary directories to
    /// avoid polluting host filesystem and ensure isolation between tests.
    Table(Arc<MountTable>),
}
