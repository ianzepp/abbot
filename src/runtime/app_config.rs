use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

use super::models_config::{ModelDef, ModelsConfig};
use crate::vfs::MountConfig;

/// Helper for deriving workspace-relative paths.
#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    pub workspace: PathBuf,
    pub root: PathBuf,
    pub mind: PathBuf,
    pub store_db: PathBuf,
    pub recall_db: PathBuf,
    pub ems_db: PathBuf,
    pub logs_db: PathBuf,
}

impl WorkspacePaths {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            root: workspace.join("root"),
            mind: workspace.join("mind"),
            store_db: workspace.join("store.db"),
            recall_db: workspace.join("recall.db"),
            ems_db: workspace.join("ems.db"),
            logs_db: workspace.join("logs.db"),
            workspace,
        }
    }
}

/// Returns the default config directory: ~/.config/abbot
pub fn config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".config").join("abbot"))
}

/// Returns the default config file path: ~/.config/abbot/abbot.toml
pub fn default_config_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("abbot.toml"))
}

/// Returns the default models file path: ~/.config/abbot/models.toml
pub fn default_models_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("models.toml"))
}

/// Derive workspace directory from a workspace root (removes /root suffix if present).
pub fn workspace_dir_from_root(workspace_root: &Path) -> PathBuf {
    let file_name = workspace_root.file_name().map(|s| s.to_string_lossy());
    if file_name.as_deref() == Some("root") {
        workspace_root
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| workspace_root.to_path_buf())
    } else {
        workspace_root.to_path_buf()
    }
}

/// Get mind memory path from workspace root.
pub fn workspace_mind_memory(workspace_root: &Path) -> PathBuf {
    workspace_dir_from_root(workspace_root)
        .join("mind")
        .join("memory.md")
}

/// Get mind self path from workspace root.
pub fn workspace_mind_self(workspace_root: &Path) -> PathBuf {
    workspace_dir_from_root(workspace_root)
        .join("mind")
        .join("self.md")
}

/// Get head memory path from workspace root.
pub fn workspace_head_memory(workspace_root: &Path, head_id: &str) -> PathBuf {
    workspace_dir_from_root(workspace_root)
        .join("head")
        .join(head_id)
        .join("memory.md")
}

/// Get workspace config path from workspace root.
pub fn workspace_config_from_root(workspace_root: &Path) -> PathBuf {
    workspace_dir_from_root(workspace_root).join("config.toml")
}

/// Get plugins config path from workspace root.
pub fn workspace_plugins_config(workspace_root: &Path) -> PathBuf {
    workspace_dir_from_root(workspace_root).join("plugins.toml")
}

/// Get the workspace name (last component of workspace dir path).
pub fn workspace_name_from_root(workspace_root: &Path) -> String {
    workspace_dir_from_root(workspace_root)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Get transcripts directory from workspace root.
pub fn workspace_transcripts_dir(workspace_root: &Path) -> PathBuf {
    workspace_dir_from_root(workspace_root)
        .join("recall")
        .join("transcripts")
}

pub fn read_optional_file(path: &Path) -> std::io::Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(path).map(Some)
}

pub fn atomic_write_file_0600(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }

    std::fs::rename(&tmp, path)?;
    Ok(())
}

static APP_CONFIG: OnceLock<AppConfig> = OnceLock::new();

/// Model configuration section in config.toml
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ModelToml {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
}

/// Root configuration loaded from config.toml
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppConfig {
    /// Absolute path to workspace directory
    pub workspace: Option<String>,
    /// Default model configuration
    #[serde(default)]
    pub model: Option<ModelToml>,
    #[serde(default)]
    pub head: HeadToml,
    #[serde(default)]
    pub hand: HandToml,
    #[serde(default)]
    pub mind: MindToml,
    #[serde(default)]
    pub pool: PoolToml,
    #[serde(default)]
    pub harness: HarnessToml,
    #[serde(default)]
    pub vfs: VfsToml,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VfsToml {
    #[serde(default)]
    pub mounts: Vec<MountConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LlmToml {
    /// Model ID in "provider/model" format (references models.toml)
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HeadToml {
    #[serde(flatten)]
    pub llm: LlmToml,
    pub heartbeat_tick: Option<u64>,
    pub debounce_ms: Option<u64>,
    /// Number of head instances in the pool (default: 3)
    pub pool: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HandToml {
    #[serde(flatten)]
    pub llm: LlmToml,
    pub max_iters: Option<usize>,
    pub max_output_chars_in_prompt: Option<usize>,
    pub max_trace_entries_in_prompt: Option<usize>,
    /// Number of hand instances in the pool (default: 4)
    pub pool: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MindToml {
    #[serde(flatten)]
    pub llm: LlmToml,
    pub tick_interval: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PoolToml {
    /// Number of concurrent hands in the pool (default: 4)
    pub size: Option<usize>,
    /// Task timeout in seconds (default: 300)
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HarnessToml {
    /// Default model in "provider/model" format, used when head/hand/mind don't specify one
    pub model: Option<String>,
    /// Ticks until slow_idle fires (default: 5, i.e. 5 minutes)
    pub slow_idle: Option<u64>,
    /// Ticks until deep_idle fires (default: 60, i.e. 60 minutes)
    pub deep_idle: Option<u64>,
}

impl HarnessToml {
    /// Load harness config from workspace config.toml via workspace root path.
    pub fn from_workspace(workspace_root: &Path) -> Self {
        let config_path = workspace_config_from_root(workspace_root);

        let config_str = match read_optional_file(&config_path) {
            Ok(Some(s)) => s,
            _ => return Self::default(),
        };

        let config: toml::Table = match config_str.parse() {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };

        let Some(harness) = config.get("harness").and_then(|v| v.as_table()) else {
            return Self::default();
        };

        Self {
            model: harness
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            slow_idle: harness
                .get("slow_idle")
                .and_then(|v| v.as_integer())
                .map(|i| i as u64),
            deep_idle: harness
                .get("deep_idle")
                .and_then(|v| v.as_integer())
                .map(|i| i as u64),
        }
    }

    /// Get slow_idle in milliseconds (default: 5 minutes)
    pub fn slow_idle_ms(&self) -> i64 {
        let ticks = self.slow_idle.unwrap_or(5);
        (ticks * 60 * 1000) as i64
    }

    /// Get deep_idle in milliseconds (default: 60 minutes)
    pub fn deep_idle_ms(&self) -> i64 {
        let ticks = self.deep_idle.unwrap_or(60);
        (ticks * 60 * 1000) as i64
    }
}

impl AppConfig {
    /// Load config from file, or return default if file doesn't exist.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        if !path.exists() {
            tracing::debug!(?path, "config file not found, using defaults");
            return Self::default();
        }

        match std::fs::read_to_string(path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => {
                    tracing::info!(?path, "loaded config");
                    config
                }
                Err(e) => {
                    tracing::warn!(?path, error = %e, "failed to parse config, using defaults");
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!(?path, error = %e, "failed to read config, using defaults");
                Self::default()
            }
        }
    }

    /// Initialize the global config. Call once at startup.
    /// Also initializes ModelsConfig from ~/.config/abbot/models.toml.
    pub fn init(path: impl AsRef<Path>) {
        let config = Self::load(path);
        let _ = APP_CONFIG.set(config);
        // Also initialize models config from ~/.config/abbot/models.toml
        if let Some(models_path) = default_models_path() {
            ModelsConfig::init(&models_path);
        } else {
            tracing::warn!("could not determine config directory, models.toml not loaded");
        }
    }

    /// Initialize the global config from the default path (~/.config/abbot/abbot.toml).
    pub fn init_default() {
        if let Some(path) = default_config_path() {
            Self::init(&path);
        } else {
            tracing::warn!("could not determine config directory, using defaults");
            let _ = APP_CONFIG.set(Self::default());
        }
    }

    /// Get the global config. Returns default if not initialized.
    /// In production, call `init()` or `init_default()` at startup.
    /// In tests, this returns defaults (no file loading).
    pub fn global() -> &'static AppConfig {
        APP_CONFIG.get_or_init(|| {
            if cfg!(test) {
                Self::default()
            } else {
                tracing::debug!("AppConfig not initialized, loading from default path");
                if let Some(path) = default_config_path() {
                    Self::load(&path)
                } else {
                    Self::default()
                }
            }
        })
    }

    /// Look up a model definition by its full ID (e.g., "openai/gpt-4.1")
    pub fn lookup_model(&self, id: &str) -> Option<&ModelDef> {
        ModelsConfig::global().get(id)
    }

    /// Get the workspace path from config.
    /// Returns error if workspace is not set, empty, or not absolute.
    pub fn workspace_path(&self) -> Result<PathBuf, String> {
        let workspace = self.workspace.as_ref().ok_or("workspace not configured")?;
        let workspace = workspace.trim();
        if workspace.is_empty() {
            return Err("workspace path is empty".to_string());
        }
        let path = PathBuf::from(workspace);
        if !path.is_absolute() {
            return Err(format!("workspace path must be absolute: {}", workspace));
        }
        Ok(path)
    }

    /// Get the default model (provider, model) from config.
    /// Returns error if [model] section is not configured.
    pub fn default_model(&self) -> Result<(&str, &str), String> {
        let model = self
            .model
            .as_ref()
            .ok_or("[model] section not configured")?;
        let provider = model.provider.as_deref().ok_or("model.provider not set")?;
        let model_name = model.model.as_deref().ok_or("model.model not set")?;
        Ok((provider, model_name))
    }

    /// Get WorkspacePaths helper from the configured workspace.
    pub fn workspace_paths(&self) -> Result<WorkspacePaths, String> {
        Ok(WorkspacePaths::new(self.workspace_path()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_toml() {
        let toml = r#"
[head]
model = "gpt-4"
temperature = 0.7
heartbeat_tick = 10

[hand]
model = "gpt-4-mini"
max_iters = 24

[mind]
model = "gpt-4"
tick_interval = 60

[pool]
size = 8
timeout_secs = 600
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.head.llm.model, Some("gpt-4".to_string()));
        assert_eq!(config.head.llm.temperature, Some(0.7));
        assert_eq!(config.head.heartbeat_tick, Some(10));
        assert_eq!(config.hand.llm.model, Some("gpt-4-mini".to_string()));
        assert_eq!(config.hand.max_iters, Some(24));
        assert_eq!(config.mind.tick_interval, Some(60));
        assert_eq!(config.pool.size, Some(8));
        assert_eq!(config.pool.timeout_secs, Some(600));
    }

    #[test]
    fn missing_fields_are_none() {
        let toml = r#"
[head]
model = "gpt-4"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.head.llm.model, Some("gpt-4".to_string()));
        assert_eq!(config.head.llm.temperature, None);
        assert_eq!(config.hand.llm.model, None);
    }

    #[test]
    fn empty_config_uses_defaults() {
        let config: AppConfig = toml::from_str("").unwrap();
        assert_eq!(config.head.llm.model, None);
        assert_eq!(config.hand.llm.model, None);
        assert_eq!(config.mind.llm.model, None);
    }

    #[test]
    fn harness_defaults() {
        let harness = HarnessToml::default();
        assert_eq!(harness.slow_idle, None);
        assert_eq!(harness.deep_idle, None);
        assert_eq!(harness.slow_idle_ms(), 5 * 60 * 1000);
        assert_eq!(harness.deep_idle_ms(), 60 * 60 * 1000);
    }

    #[test]
    fn harness_custom_ticks() {
        let harness = HarnessToml {
            model: None,
            slow_idle: Some(10),
            deep_idle: Some(120),
        };
        assert_eq!(harness.slow_idle_ms(), 10 * 60 * 1000);
        assert_eq!(harness.deep_idle_ms(), 120 * 60 * 1000);
    }

    #[test]
    fn harness_with_model() {
        let harness = HarnessToml {
            model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
            slow_idle: None,
            deep_idle: None,
        };
        assert_eq!(
            harness.model.as_deref(),
            Some("anthropic/claude-sonnet-4-20250514")
        );
    }

    #[test]
    fn workspace_path_valid() {
        let toml = r#"
workspace = "/path/to/workspace"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            config.workspace_path().unwrap(),
            PathBuf::from("/path/to/workspace")
        );
    }

    #[test]
    fn workspace_path_missing() {
        let config: AppConfig = toml::from_str("").unwrap();
        assert!(config.workspace_path().is_err());
    }

    #[test]
    fn workspace_path_empty() {
        let toml = r#"workspace = """#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert!(config.workspace_path().is_err());
    }

    #[test]
    fn workspace_path_relative_rejected() {
        let toml = r#"workspace = "relative/path""#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        let err = config.workspace_path().unwrap_err();
        assert!(err.contains("absolute"));
    }

    #[test]
    fn default_model_valid() {
        let toml = r#"
[model]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        let (provider, model) = config.default_model().unwrap();
        assert_eq!(provider, "anthropic");
        assert_eq!(model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn default_model_missing_section() {
        let config: AppConfig = toml::from_str("").unwrap();
        assert!(config.default_model().is_err());
    }

    #[test]
    fn workspace_paths_helper() {
        let toml = r#"workspace = "/my/workspace""#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        let paths = config.workspace_paths().unwrap();
        assert_eq!(paths.workspace, PathBuf::from("/my/workspace"));
        assert_eq!(paths.root, PathBuf::from("/my/workspace/root"));
        assert_eq!(paths.mind, PathBuf::from("/my/workspace/mind"));
        assert_eq!(paths.store_db, PathBuf::from("/my/workspace/store.db"));
        assert_eq!(paths.recall_db, PathBuf::from("/my/workspace/recall.db"));
        assert_eq!(paths.ems_db, PathBuf::from("/my/workspace/ems.db"));
        assert_eq!(paths.logs_db, PathBuf::from("/my/workspace/logs.db"));
    }
}
