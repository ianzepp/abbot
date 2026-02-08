//! VFS Mount Table - Virtual Filesystem Mount Management
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module implements a virtual filesystem (VFS) layer that maps guest paths
//! to host filesystem paths via mount points. Unmatched paths fall through to an
//! in-memory filesystem (MemoryFs), giving agents scratch space without exposing
//! the host filesystem.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Mount-based access: All filesystem access goes through mount points,
//!   making it impossible to access host paths outside configured mounts
//! - Longest-prefix matching: Multiple mounts can coexist; the most specific
//!   (longest) prefix wins, enabling nested mount hierarchies
//! - Memory-backed root: Unmatched paths resolve to MemoryFs, providing safe
//!   scratch space without exposing the host filesystem
//! - Symlink escape detection: Warns when symlinks escape mount boundaries,
//!   providing visibility without breaking legitimate use cases
//!
//! SECURITY MODEL
//! ==============
//! - Root mount forbidden: "/" prefix is rejected in config — the VFS root is
//!   always memory-backed, ensuring the host filesystem is never exposed by default
//! - Path normalization: Guest paths are normalized to prevent directory
//!   traversal attacks (/../etc/passwd)
//! - Symlink escape warnings: When symlinks resolve outside mount boundaries,
//!   warnings are logged (but access is allowed to support shared libs, etc.)
//! - Read-only enforcement: Mounts can be marked ro (read-only), preventing
//!   writes to sensitive directories

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::kernel::KernelError;

use super::config::MountConfig;
use super::memory::MemoryFs;
use super::path::{expand_host_path, normalize_path};

// =============================================================================
// TYPES
// =============================================================================

/// Access mode for a mount point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MountMode {
    Ro,
    #[default]
    Rw,
}

/// A runtime mount point linking a VFS prefix to a host path.
#[derive(Debug, Clone)]
pub struct HostMount {
    pub prefix: String,
    pub host_path: PathBuf,
    pub mode: MountMode,
}

/// Result of resolving a VFS path to a host path.
#[derive(Debug, Clone)]
pub struct ResolvedPath {
    pub host_path: PathBuf,
    pub mount: HostMount,
}

/// Result of VFS path resolution — either a host mount or the memory filesystem.
#[derive(Debug, Clone)]
pub enum VfsResolution {
    /// Path resolved to a host mount point.
    Host(ResolvedPath),
    /// Path falls through to the in-memory filesystem.
    Memory { path: String, memory: MemoryFs },
}

// =============================================================================
// MOUNT TABLE
// =============================================================================

/// The mount table maps VFS paths to host paths, with MemoryFs as fallback root.
#[derive(Debug, Clone)]
pub struct MountTable {
    mounts: Vec<HostMount>,
    memory: MemoryFs,
    /// Optional persistent sandbox root (e.g. `~/.abbot/sandbox/`).
    /// Catches unmatched paths before MemoryFs fallback, giving agents persistent
    /// scratch space that survives process restarts.
    root_mount: Option<HostMount>,
}

/// Global mount table singleton — always initialized (no Option needed).
static MOUNT_TABLE: OnceLock<MountTable> = OnceLock::new();

impl MountTable {
    /// Build a MountTable from a list of configs.
    ///
    /// Rejects "/" prefix in user configs — the VFS root is not user-configurable.
    /// An optional `sandbox` path provides a persistent host-backed root that
    /// catches unmatched paths before the ephemeral MemoryFs fallback.
    pub fn from_config(
        configs: Vec<MountConfig>,
        sandbox: Option<PathBuf>,
    ) -> Result<Self, KernelError> {
        let mut mounts = Vec::with_capacity(configs.len());
        let mut seen_prefixes = HashSet::new();

        for cfg in configs {
            let prefix = normalize_path(&cfg.prefix)?.to_string_lossy().to_string();

            // Reject root mount in user configs — users cannot override root via toml
            if prefix == "/" {
                return Err(KernelError::invalid_args(
                    "root mount not allowed in config; use sandbox instead",
                ));
            }

            if !seen_prefixes.insert(prefix.clone()) {
                return Err(KernelError::invalid_args(format!(
                    "duplicate mount prefix: {}",
                    prefix
                )));
            }

            let host_path = expand_host_path(&cfg.host)?;

            mounts.push(HostMount {
                prefix,
                host_path,
                mode: cfg.mode,
            });
        }

        // Sort descending by prefix length for longest-prefix matching
        mounts.sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()));

        // Build the sandbox root mount if a path was provided
        let root_mount = sandbox.map(|path| HostMount {
            prefix: "/".to_string(),
            host_path: path,
            mode: MountMode::Rw,
        });

        Ok(Self {
            mounts,
            memory: MemoryFs::new(),
            root_mount,
        })
    }

    /// Resolve a VFS path — returns Host if a mount matches, Memory otherwise.
    pub fn resolve(&self, vfs_path: &str) -> Result<VfsResolution, KernelError> {
        let normalized = normalize_path(vfs_path)?;
        let normalized_str = normalized.to_string_lossy();

        // Try host mounts first (longest-prefix match)
        for mount in &self.mounts {
            if normalized_str.starts_with(&mount.prefix) {
                let suffix = normalized_str
                    .strip_prefix(&mount.prefix)
                    .unwrap_or(&normalized_str)
                    .trim_start_matches('/');

                let host_path = if suffix.is_empty() {
                    mount.host_path.clone()
                } else {
                    mount.host_path.join(suffix)
                };

                // Symlink escape detection (warn only)
                if let Ok(canonical) = host_path.canonicalize()
                    && !canonical.starts_with(&mount.host_path)
                {
                    tracing::warn!(
                        vfs_path = %vfs_path,
                        host_path = %host_path.display(),
                        canonical = %canonical.display(),
                        mount_root = %mount.host_path.display(),
                        "symlink escapes mount boundary"
                    );
                }

                return Ok(VfsResolution::Host(ResolvedPath {
                    host_path,
                    mount: mount.clone(),
                }));
            }
        }

        // Try sandbox root mount before memory fallback
        if let Some(root) = &self.root_mount {
            let suffix = normalized_str.trim_start_matches('/');
            let host_path = if suffix.is_empty() {
                root.host_path.clone()
            } else {
                root.host_path.join(suffix)
            };

            return Ok(VfsResolution::Host(ResolvedPath {
                host_path,
                mount: root.clone(),
            }));
        }

        // Fall through to memory filesystem
        Ok(VfsResolution::Memory {
            path: normalized_str.to_string(),
            memory: self.memory.clone(),
        })
    }

    /// Get a reference to the memory filesystem.
    pub fn memory(&self) -> &MemoryFs {
        &self.memory
    }

    /// Returns the list of host mounts (for bundle builders, VFS-aware context).
    pub fn host_mounts(&self) -> &[HostMount] {
        &self.mounts
    }

    /// Returns sorted list of host mount prefixes (for `fs:list /`).
    pub fn mount_prefixes(&self) -> Vec<String> {
        let mut prefixes: Vec<String> = self.mounts.iter().map(|m| m.prefix.clone()).collect();
        prefixes.sort();
        prefixes
    }

    /// Initialize the global mount table. Call once at startup.
    /// Always creates a MountTable (with MemoryFs root), even if no host mounts configured.
    /// If `sandbox` is Some, unmatched paths resolve to that host directory instead of MemoryFs.
    pub fn init(configs: Vec<MountConfig>, sandbox: Option<PathBuf>) -> Result<(), KernelError> {
        let table = Self::from_config(configs, sandbox)?;
        let _ = MOUNT_TABLE.set(table);
        Ok(())
    }

    /// Get the global mount table. Always present after init().
    pub fn global() -> &'static MountTable {
        MOUNT_TABLE
            .get()
            .expect("MountTable::init() must be called before MountTable::global()")
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
    fn test_root_mount_rejected() {
        let configs = vec![make_config("/", "~/project", MountMode::Rw)];
        let err = MountTable::from_config(configs, None).unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
        assert!(err.message.contains("root mount not allowed"));
    }

    #[test]
    fn test_mount_table_from_config() {
        let configs = vec![
            make_config("/project", "~/project", MountMode::Rw),
            make_config("/data", "/data/shared", MountMode::Ro),
        ];
        let table = MountTable::from_config(configs, None).unwrap();
        assert_eq!(table.mounts.len(), 2);
        // Sorted by descending prefix length
        assert_eq!(table.mounts[0].prefix, "/project");
        assert_eq!(table.mounts[1].prefix, "/data");
    }

    #[test]
    fn test_duplicate_prefix_rejected() {
        let configs = vec![
            make_config("/data", "/data1", MountMode::Rw),
            make_config("/data", "/data2", MountMode::Rw),
        ];
        let err = MountTable::from_config(configs, None).unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
        assert!(err.message.contains("duplicate"));
    }

    #[test]
    fn test_resolve_host_mount() {
        let configs = vec![make_config("/project", "/home/user/project", MountMode::Rw)];
        let table = MountTable::from_config(configs, None).unwrap();

        let res = table.resolve("/project/foo/bar.txt").unwrap();
        match res {
            VfsResolution::Host(resolved) => {
                assert_eq!(
                    resolved.host_path,
                    PathBuf::from("/home/user/project/foo/bar.txt")
                );
                assert_eq!(resolved.mount.mode, MountMode::Rw);
            }
            VfsResolution::Memory { .. } => panic!("expected Host resolution"),
        }
    }

    #[test]
    fn test_resolve_falls_through_to_memory() {
        let configs = vec![make_config("/project", "/home/user/project", MountMode::Rw)];
        let table = MountTable::from_config(configs, None).unwrap();

        let res = table.resolve("/scratch/notes.txt").unwrap();
        match res {
            VfsResolution::Memory { path, .. } => {
                assert_eq!(path, "/scratch/notes.txt");
            }
            VfsResolution::Host(_) => panic!("expected Memory resolution"),
        }
    }

    #[test]
    fn test_resolve_root_falls_through_to_memory() {
        let table = MountTable::from_config(vec![], None).unwrap();
        let res = table.resolve("/anything").unwrap();
        assert!(matches!(res, VfsResolution::Memory { .. }));
    }

    #[test]
    fn test_resolve_dotdot_escape_rejected() {
        let configs = vec![make_config("/project", "/home/user/project", MountMode::Rw)];
        let table = MountTable::from_config(configs, None).unwrap();

        let err = table.resolve("/project/../../etc/passwd").unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[test]
    fn test_mount_prefixes() {
        let configs = vec![
            make_config("/project", "~/project", MountMode::Rw),
            make_config("/data", "/data/shared", MountMode::Ro),
        ];
        let table = MountTable::from_config(configs, None).unwrap();
        let prefixes = table.mount_prefixes();
        assert_eq!(prefixes, vec!["/data", "/project"]);
    }

    #[tokio::test]
    async fn test_empty_config_creates_table() {
        let table = MountTable::from_config(vec![], None).unwrap();
        assert!(table.mounts.is_empty());
        // Memory is still available
        assert!(table.memory().exists("/").await);
    }

    #[test]
    fn test_nested_mount_precedence() {
        let configs = vec![
            make_config("/data", "/data/shared", MountMode::Ro),
            make_config("/data/rw", "/data/writable", MountMode::Rw),
        ];
        let table = MountTable::from_config(configs, None).unwrap();

        // /data/rw/file should match the more specific mount
        let res = table.resolve("/data/rw/file.txt").unwrap();
        match res {
            VfsResolution::Host(resolved) => {
                assert_eq!(resolved.host_path, PathBuf::from("/data/writable/file.txt"));
                assert_eq!(resolved.mount.mode, MountMode::Rw);
            }
            VfsResolution::Memory { .. } => panic!("expected Host resolution"),
        }

        // /data/file should match the less specific mount
        let res2 = table.resolve("/data/file.txt").unwrap();
        match res2 {
            VfsResolution::Host(resolved) => {
                assert_eq!(resolved.host_path, PathBuf::from("/data/shared/file.txt"));
                assert_eq!(resolved.mount.mode, MountMode::Ro);
            }
            VfsResolution::Memory { .. } => panic!("expected Host resolution"),
        }
    }

    #[test]
    fn test_sandbox_root_resolves_to_host() {
        let sandbox = PathBuf::from("/tmp/sandbox");
        let table = MountTable::from_config(vec![], Some(sandbox)).unwrap();

        // Unmatched path should resolve to sandbox host path
        let res = table.resolve("/docs/test.md").unwrap();
        match res {
            VfsResolution::Host(resolved) => {
                assert_eq!(
                    resolved.host_path,
                    PathBuf::from("/tmp/sandbox/docs/test.md")
                );
                assert_eq!(resolved.mount.mode, MountMode::Rw);
                assert_eq!(resolved.mount.prefix, "/");
            }
            VfsResolution::Memory { .. } => panic!("expected Host resolution"),
        }
    }

    #[test]
    fn test_sandbox_explicit_mount_takes_precedence() {
        let sandbox = PathBuf::from("/tmp/sandbox");
        let configs = vec![make_config("/project", "/home/user/project", MountMode::Rw)];
        let table = MountTable::from_config(configs, Some(sandbox)).unwrap();

        // /project path should still match the explicit mount, not sandbox
        let res = table.resolve("/project/src/main.rs").unwrap();
        match res {
            VfsResolution::Host(resolved) => {
                assert_eq!(
                    resolved.host_path,
                    PathBuf::from("/home/user/project/src/main.rs")
                );
                assert_eq!(resolved.mount.prefix, "/project");
            }
            VfsResolution::Memory { .. } => panic!("expected Host resolution"),
        }

        // Unmatched path goes to sandbox
        let res2 = table.resolve("/scratch/notes.txt").unwrap();
        match res2 {
            VfsResolution::Host(resolved) => {
                assert_eq!(
                    resolved.host_path,
                    PathBuf::from("/tmp/sandbox/scratch/notes.txt")
                );
                assert_eq!(resolved.mount.prefix, "/");
            }
            VfsResolution::Memory { .. } => panic!("expected Host resolution"),
        }
    }
}
