//! VFS Mount Table - Virtual Filesystem Mount Management
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module implements a virtual filesystem (VFS) layer that maps guest paths
//! to host filesystem paths via mount points. The VFS provides filesystem access
//! isolation, allowing controlled exposure of host directories to the kernel's
//! syscall implementations.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Mount-based access: All filesystem access goes through mount points,
//!   making it impossible to access host paths outside configured mounts
//! - Longest-prefix matching: Multiple mounts can coexist; the most specific
//!   (longest) prefix wins, enabling nested mount hierarchies
//! - Fail-secure defaults: Empty mount table disables filesystem access entirely
//! - Symlink escape detection: Warns when symlinks escape mount boundaries,
//!   providing visibility without breaking legitimate use cases
//!
//! SECURITY MODEL
//! ==============
//! - No access without mounts: If no mounts are configured, all filesystem
//!   access is denied with E_DISABLED
//! - Path normalization: Guest paths are normalized to prevent directory
//!   traversal attacks (/../etc/passwd)
//! - Symlink escape warnings: When symlinks resolve outside mount boundaries,
//!   warnings are logged (but access is allowed to support shared libs, etc.)
//! - Read-only enforcement: Mounts can be marked ro (read-only), preventing
//!   writes to sensitive directories
//!
//! MOUNT RESOLUTION ALGORITHM
//! ==========================
//! 1. Normalize the guest path (remove .., ., //, etc.)
//! 2. Find the mount with the longest matching prefix
//! 3. Strip the mount prefix from the guest path to get the suffix
//! 4. Join mount.host_path with the suffix to get the final host path
//! 5. Optionally canonicalize and check for symlink escapes (warn only)
//!
//! TRADE-OFFS
//! ==========
//! - Symlink escape warnings only: We log when symlinks escape mounts but don't
//!   block access. Blocking would break legitimate cases (shared libraries,
//!   system headers), but warnings provide visibility for unexpected escapes.
//! - Global singleton: Mount table uses OnceLock for global access, trading
//!   flexibility (no runtime reconfiguration) for simplicity and performance.
//! - Prefix-based only: No support for glob patterns or complex rules; mount
//!   points are simple prefix matches. Keeps the system understandable.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::kernel::KernelError;

use super::config::MountConfig;
use super::path::{expand_host_path, normalize_path};

// =============================================================================
// TYPES
// =============================================================================

/// Access mode for a mount point.
///
/// WHY enum: Makes access control explicit in configuration and prevents
/// typos (ro/rw vs. read-only/readwrite/readonly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MountMode {
    Ro,
    #[default]
    Rw,
}

/// A runtime mount point linking a VFS prefix to a host path.
///
/// WHY separate from MountConfig: The config type is for deserialization;
/// this type is for runtime use with validated, expanded paths.
#[derive(Debug, Clone)]
pub struct HostMount {
    pub prefix: String,
    pub host_path: PathBuf,
    pub mode: MountMode,
}

/// Result of resolving a VFS path to a host path.
///
/// WHY include mount: Callers need to check mount.mode to enforce read-only
/// restrictions on write operations.
#[derive(Debug, Clone)]
pub struct ResolvedPath {
    pub host_path: PathBuf,
    pub mount: HostMount,
}

// =============================================================================
// MOUNT TABLE
// =============================================================================

/// The mount table maps VFS paths to host paths.
///
/// WHY sorted by prefix length: Longest-prefix matching ensures that more
/// specific mounts (e.g., "/data/shared") take precedence over general mounts
/// (e.g., "/data") when resolving paths.
#[derive(Debug, Clone)]
pub struct MountTable {
    mounts: Vec<HostMount>,
}

/// Global mount table singleton.
///
/// WHY OnceLock: Mount table is initialized once at startup and never changes.
/// OnceLock provides thread-safe initialization without runtime overhead.
///
/// WHY Option: Allows representing "no mounts configured" as None, which
/// cleanly maps to E_DISABLED errors when filesystem access is attempted.
static MOUNT_TABLE: OnceLock<Option<MountTable>> = OnceLock::new();

impl MountTable {
    /// Build a MountTable from a list of configs.
    ///
    /// WHY validation during construction: Catches configuration errors
    /// (duplicate prefixes, invalid paths) at startup rather than during
    /// first filesystem access.
    ///
    /// WHY sort by prefix length descending: Enables efficient longest-prefix
    /// matching in resolve() by checking longer prefixes first.
    pub fn from_config(configs: Vec<MountConfig>) -> Result<Self, KernelError> {
        let mut mounts = Vec::with_capacity(configs.len());
        let mut seen_prefixes = HashSet::new();

        for cfg in configs {
            // WHY normalize prefix: Ensures consistent prefix format (/foo, not
            // /foo/, /foo/., etc.) for reliable prefix matching
            let prefix = normalize_path(&cfg.prefix)?.to_string_lossy().to_string();

            // WHY reject duplicates: Multiple mounts with the same prefix would
            // create ambiguity about which mount to use
            if !seen_prefixes.insert(prefix.clone()) {
                return Err(KernelError::invalid_args(format!(
                    "duplicate mount prefix: {}",
                    prefix
                )));
            }

            // WHY expand host path: Resolves ~ and validates path format at
            // initialization, failing fast on configuration errors
            let host_path = expand_host_path(&cfg.host)?;

            mounts.push(HostMount {
                prefix,
                host_path,
                mode: cfg.mode,
            });
        }

        // WHY sort descending: Longest prefixes first for longest-prefix matching
        mounts.sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()));

        Ok(Self { mounts })
    }

    /// Resolve a VFS path to a host path and its mount.
    ///
    /// WHY return ResolvedPath: Bundles host path with the mount that resolved
    /// it, allowing callers to check mount.mode for access control.
    ///
    /// SECURITY: This is the core security boundary. All filesystem access must
    /// go through this function to enforce mount isolation.
    pub fn resolve(&self, vfs_path: &str) -> Result<ResolvedPath, KernelError> {
        // -------------------------------------------------------------------------
        // PHASE 1: EMPTY TABLE CHECK
        // Fail fast if filesystem access is disabled
        // -------------------------------------------------------------------------
        if self.mounts.is_empty() {
            return Err(KernelError::disabled(
                "filesystem access disabled: no mounts configured",
            ));
        }

        // -------------------------------------------------------------------------
        // PHASE 2: PATH NORMALIZATION
        // Remove .., ., // to prevent directory traversal
        // -------------------------------------------------------------------------
        let normalized = normalize_path(vfs_path)?;
        let normalized_str = normalized.to_string_lossy();

        // -------------------------------------------------------------------------
        // PHASE 3: LONGEST-PREFIX MATCH
        // Find the most specific mount for this path
        // -------------------------------------------------------------------------
        for mount in &self.mounts {
            if normalized_str.starts_with(&mount.prefix) {
                // WHY special-case root mount: Strip leading slash from suffix
                // to avoid double slash (mount=/foo, suffix=/bar → /foo/bar not /foo//bar)
                let suffix = if mount.prefix == "/" {
                    normalized_str.strip_prefix('/').unwrap_or(&normalized_str)
                } else {
                    normalized_str
                        .strip_prefix(&mount.prefix)
                        .unwrap_or(&normalized_str)
                        .trim_start_matches('/')
                };

                // WHY handle empty suffix: Accessing the mount point itself
                // (e.g., "/" when mounted to "~/project") should resolve to
                // the mount root, not mount_root + "/"
                let host_path = if suffix.is_empty() {
                    mount.host_path.clone()
                } else {
                    mount.host_path.join(suffix)
                };

                // -------------------------------------------------------------------------
                // PHASE 4: SYMLINK ESCAPE DETECTION
                // Warn if symlinks resolve outside the mount boundary
                // -------------------------------------------------------------------------
                // WHY canonicalize: Resolves symlinks to detect escapes. Failures
                // are ignored (file might not exist yet, or permissions prevent access).
                if let Ok(canonical) = host_path.canonicalize() {
                    if !canonical.starts_with(&mount.host_path) {
                        // WHY warn not error: Some legitimate use cases need symlink
                        // escapes (e.g., /usr/lib → /lib, shared headers). Warnings
                        // provide visibility without breaking these cases.
                        tracing::warn!(
                            vfs_path = %vfs_path,
                            host_path = %host_path.display(),
                            canonical = %canonical.display(),
                            mount_root = %mount.host_path.display(),
                            "symlink escapes mount boundary"
                        );
                    }
                }

                return Ok(ResolvedPath {
                    host_path,
                    mount: mount.clone(),
                });
            }
        }

        // -------------------------------------------------------------------------
        // PHASE 5: NO MOUNT FOUND
        // Path doesn't match any configured mount
        // -------------------------------------------------------------------------
        Err(KernelError::forbidden(format!(
            "no mount found for path: {}",
            vfs_path
        )))
    }

    /// Check if filesystem access is disabled (no mounts configured).
    ///
    /// WHY: Allows callers to provide better error messages or skip filesystem
    /// operations entirely when the VFS is disabled.
    pub fn is_disabled(&self) -> bool {
        self.mounts.is_empty()
    }

    /// Initialize the global mount table. Call once at startup.
    ///
    /// WHY OnceLock: Ensures initialization happens exactly once, preventing
    /// race conditions or accidental reinitialization.
    ///
    /// WHY Option: Allows representing "no mounts" (empty config) as None,
    /// which cleanly maps to disabled filesystem access.
    pub fn init(configs: Vec<MountConfig>) -> Result<(), KernelError> {
        let table = if configs.is_empty() {
            None
        } else {
            Some(Self::from_config(configs)?)
        };
        let _ = MOUNT_TABLE.set(table);
        Ok(())
    }

    /// Get the global mount table, if configured.
    ///
    /// WHY static lifetime: Mount table never changes after initialization,
    /// so returning a static reference is safe and avoids reference counting.
    pub fn global() -> Option<&'static MountTable> {
        MOUNT_TABLE.get().and_then(|opt| opt.as_ref())
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(prefix: &str, host: &str, mode: MountMode) -> MountConfig {
        MountConfig {
            prefix: prefix.to_string(),
            host: host.to_string(),
            mode,
        }
    }

    #[test]
    fn test_mount_table_from_config() {
        let configs = vec![
            make_config("/", "~/project", MountMode::Rw),
            make_config("/data", "/data/shared", MountMode::Ro),
        ];
        let table = MountTable::from_config(configs).unwrap();
        assert_eq!(table.mounts.len(), 2);
        // WHY /data first: Sorted by descending prefix length
        assert_eq!(table.mounts[0].prefix, "/data");
        assert_eq!(table.mounts[1].prefix, "/");
    }

    #[test]
    fn test_duplicate_prefix_rejected() {
        let configs = vec![
            make_config("/", "~/project1", MountMode::Rw),
            make_config("/", "~/project2", MountMode::Rw),
        ];
        let err = MountTable::from_config(configs).unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
        assert!(err.message.contains("duplicate"));
    }

    #[test]
    fn test_resolve_root_mount() {
        let configs = vec![make_config("/", "/home/user/project", MountMode::Rw)];
        let table = MountTable::from_config(configs).unwrap();

        let resolved = table.resolve("/foo/bar.txt").unwrap();
        assert_eq!(
            resolved.host_path,
            PathBuf::from("/home/user/project/foo/bar.txt")
        );
        assert_eq!(resolved.mount.mode, MountMode::Rw);
    }

    #[test]
    fn test_resolve_nested_mount() {
        let configs = vec![
            make_config("/", "/home/user/project", MountMode::Rw),
            make_config("/data", "/data/shared", MountMode::Ro),
        ];
        let table = MountTable::from_config(configs).unwrap();

        let resolved = table.resolve("/data/file.txt").unwrap();
        assert_eq!(resolved.host_path, PathBuf::from("/data/shared/file.txt"));
        assert_eq!(resolved.mount.mode, MountMode::Ro);

        let resolved2 = table.resolve("/src/main.rs").unwrap();
        assert_eq!(
            resolved2.host_path,
            PathBuf::from("/home/user/project/src/main.rs")
        );
        assert_eq!(resolved2.mount.mode, MountMode::Rw);
    }

    #[test]
    fn test_resolve_empty_table_disabled() {
        let table = MountTable::from_config(vec![]).unwrap();
        let err = table.resolve("/foo").unwrap_err();
        assert_eq!(err.code, "E_DISABLED");
    }

    #[test]
    fn test_resolve_no_matching_mount() {
        let configs = vec![make_config("/data", "/data/shared", MountMode::Ro)];
        let table = MountTable::from_config(configs).unwrap();

        let err = table.resolve("/foo/bar").unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[test]
    fn test_resolve_dotdot_escape_rejected() {
        let configs = vec![make_config("/", "/home/user/project", MountMode::Rw)];
        let table = MountTable::from_config(configs).unwrap();

        let err = table.resolve("/foo/../../etc/passwd").unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[test]
    fn test_is_disabled() {
        let empty = MountTable::from_config(vec![]).unwrap();
        assert!(empty.is_disabled());

        let non_empty =
            MountTable::from_config(vec![make_config("/", "/home", MountMode::Rw)]).unwrap();
        assert!(!non_empty.is_disabled());
    }
}
