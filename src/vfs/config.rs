use serde::Deserialize;

use super::mount::MountMode;

#[derive(Debug, Clone, Deserialize)]
pub struct MountConfig {
    pub prefix: String,
    pub host: String,
    #[serde(default)]
    pub mode: MountMode,
}

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
