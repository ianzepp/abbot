//! VFS Configuration - Mount Point Configuration Types
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module defines the configuration types for VFS mount points. Mount
//! configurations specify how guest (VFS) paths map to host filesystem paths,
//! along with access modes (read-only or read-write).
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Declarative configuration: Mount points are defined in TOML configuration
//!   files, keeping filesystem access policy separate from code
//! - Explicit modes: Access modes (ro/rw) are explicit, preventing accidental
//!   write access to sensitive host directories
//! - Simple structure: Minimal configuration fields (prefix, host, mode) keep
//!   the system understandable and maintainable

use serde::Deserialize;

use super::mount::MountMode;

// =============================================================================
// CONFIGURATION TYPE
// =============================================================================

/// Configuration for a single mount point.
///
/// WHY separate config type: Decouples TOML deserialization from the runtime
/// mount representation (HostMount), allowing config validation and
/// transformation during mount table initialization.
#[derive(Debug, Clone, Deserialize)]
pub struct MountConfig {
    /// VFS path prefix (e.g., "/", "/data")
    ///
    /// WHY: Defines the guest-side path that this mount will handle. Paths
    /// starting with this prefix are mapped to the host path.
    pub prefix: String,

    /// Host filesystem path (supports ~ expansion)
    ///
    /// WHY: The actual host directory to expose. Supports "~" for home
    /// directory expansion, making configs more portable across users.
    pub host: String,

    /// Access mode (ro or rw), defaults to rw
    ///
    /// WHY default rw: Most mounts are for the user's working directory where
    /// read-write access is expected. Read-only is opt-in for safety-critical
    /// mounts like shared data directories.
    #[serde(default)]
    pub mode: MountMode,
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mount_config_deserialize() {
        let toml = r#"
            prefix = "/"
            host = "~/github/project"
        "#;
        let cfg: MountConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.prefix, "/");
        assert_eq!(cfg.host, "~/github/project");
        assert_eq!(cfg.mode, MountMode::Rw);
    }

    #[test]
    fn test_mount_config_with_mode() {
        let toml = r#"
            prefix = "/shared"
            host = "/data/shared"
            mode = "ro"
        "#;
        let cfg: MountConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.prefix, "/shared");
        assert_eq!(cfg.host, "/data/shared");
        assert_eq!(cfg.mode, MountMode::Ro);
    }

    #[test]
    fn test_mount_config_array() {
        let toml = r#"
            [[mounts]]
            prefix = "/"
            host = "~/github/project"

            [[mounts]]
            prefix = "/shared"
            host = "/data/shared"
            mode = "ro"
        "#;

        #[derive(Deserialize)]
        struct Container {
            mounts: Vec<MountConfig>,
        }

        let cfg: Container = toml::from_str(toml).unwrap();
        assert_eq!(cfg.mounts.len(), 2);
        assert_eq!(cfg.mounts[0].prefix, "/");
        assert_eq!(cfg.mounts[1].mode, MountMode::Ro);
    }
}
