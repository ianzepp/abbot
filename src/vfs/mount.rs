use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::kernel::KernelError;

use super::config::MountConfig;
use super::path::{expand_host_path, normalize_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MountMode {
    Ro,
    #[default]
    Rw,
}

#[derive(Debug, Clone)]
pub struct HostMount {
    pub prefix: String,
    pub host_path: PathBuf,
    pub mode: MountMode,
}

#[derive(Debug, Clone)]
pub struct ResolvedPath {
    pub host_path: PathBuf,
    pub mount: HostMount,
}

#[derive(Debug, Clone)]
pub struct MountTable {
    mounts: Vec<HostMount>,
}

static MOUNT_TABLE: OnceLock<Option<MountTable>> = OnceLock::new();

impl MountTable {
    /// Build a MountTable from a list of configs.
    /// Validates prefixes and expands host paths.
    pub fn from_config(configs: Vec<MountConfig>) -> Result<Self, KernelError> {
        let mut mounts = Vec::with_capacity(configs.len());
        let mut seen_prefixes = HashSet::new();

        for cfg in configs {
            let prefix = normalize_path(&cfg.prefix)?
                .to_string_lossy()
                .to_string();

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

        mounts.sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()));

        Ok(Self { mounts })
    }

    /// Resolve a VFS path to a host path and its mount.
    pub fn resolve(&self, vfs_path: &str) -> Result<ResolvedPath, KernelError> {
        if self.mounts.is_empty() {
            return Err(KernelError::disabled("filesystem access disabled: no mounts configured"));
        }

        let normalized = normalize_path(vfs_path)?;
        let normalized_str = normalized.to_string_lossy();

        for mount in &self.mounts {
            if normalized_str.starts_with(&mount.prefix) {
                let suffix = if mount.prefix == "/" {
                    normalized_str.strip_prefix('/').unwrap_or(&normalized_str)
                } else {
                    normalized_str
                        .strip_prefix(&mount.prefix)
                        .unwrap_or(&normalized_str)
                        .trim_start_matches('/')
                };

                let host_path = if suffix.is_empty() {
                    mount.host_path.clone()
                } else {
                    mount.host_path.join(suffix)
                };

                return Ok(ResolvedPath {
                    host_path,
                    mount: mount.clone(),
                });
            }
        }

        Err(KernelError::forbidden(format!(
            "no mount found for path: {}",
            vfs_path
        )))
    }

    pub fn is_disabled(&self) -> bool {
        self.mounts.is_empty()
    }

    /// Initialize the global mount table. Call once at startup.
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
    pub fn global() -> Option<&'static MountTable> {
        MOUNT_TABLE.get().and_then(|opt| opt.as_ref())
    }
}

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
        assert_eq!(resolved.host_path, PathBuf::from("/home/user/project/foo/bar.txt"));
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
        assert_eq!(resolved2.host_path, PathBuf::from("/home/user/project/src/main.rs"));
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

        let non_empty = MountTable::from_config(vec![
            make_config("/", "/home", MountMode::Rw),
        ]).unwrap();
        assert!(!non_empty.is_disabled());
    }
}
