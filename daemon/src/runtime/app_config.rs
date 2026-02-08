use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::vfs::MountConfig;

/// Helper for deriving paths from the user's home directory.
///
/// `home` is `~` (agent CWD, VFS root).
/// `workspace` is `~/.abbot/` (data directory).
#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    pub home: PathBuf,
    pub workspace: PathBuf,
    pub mind: PathBuf,
    pub sandbox: PathBuf,
    pub store_db: PathBuf,
    pub ems_db: PathBuf,
    pub frames_db: PathBuf,
    #[cfg(unix)]
    pub frames_sock: PathBuf,
}

impl WorkspacePaths {
    pub fn new(home: PathBuf) -> Self {
        let ws = home.join(".abbot");
        Self {
            mind: ws.join("mind"),
            sandbox: ws.join("sandbox"),
            store_db: ws.join("store.db"),
            ems_db: ws.join("ems.db"),
            frames_db: ws.join("frames.db"),
            #[cfg(unix)]
            frames_sock: ws.join("frames.sock"),
            workspace: ws,
            home,
        }
    }
}

/// Returns the default config/data directory: ~/.abbot
pub fn config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".abbot"))
}

/// Returns the default config file path: ~/.abbot/abbot.toml
pub fn default_config_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("abbot.toml"))
}

/// Returns the path to the API keys file: ~/.abbot/keys.env
pub fn keys_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("keys.env"))
}

/// Returns the provider cache directory: ~/.abbot/providers/
pub fn providers_dir() -> Option<PathBuf> {
    config_dir().map(|p| p.join("providers"))
}

/// Returns the default frames database path: ~/.abbot/frames.db
pub fn default_frames_db_path() -> Option<PathBuf> {
    config_dir().map(|p| p.join("frames.db"))
}

/// Derive the data directory (~/.abbot/) from the home directory.
pub fn workspace_dir_from_root(home: &Path) -> PathBuf {
    home.join(".abbot")
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

/// Provider connection configuration.
///
/// Secrets are not stored here; `api_key_env` points at an env var (typically
/// loaded from ~/.abbot/keys.env at startup).
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderToml {
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
}

/// Global trait selections. Each field is a trait category; the value is the
/// selected variant name (e.g. "mild") or "none" to disable.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TraitsToml {
    pub fever: Option<String>,
    pub generation: Option<String>,
    pub autist: Option<String>,
    pub filter: Option<String>,
    pub poverty: Option<String>,
    pub ego: Option<String>,
    pub paranoia: Option<String>,
    pub cultist: Option<String>,
    pub dominance: Option<String>,
    pub bipolar: Option<String>,
    pub xenophobe: Option<String>,
    pub esoteric: Option<String>,
    pub collab: Option<String>,
}

impl TraitsToml {
    /// Convert selected traits to `["category/variant", ...]` format for `render_traits()`.
    /// Entries set to "none" or empty are excluded.
    pub fn to_trait_names(&self) -> Vec<String> {
        let entries: &[(&str, &Option<String>)] = &[
            ("fever", &self.fever),
            ("generation", &self.generation),
            ("autist", &self.autist),
            ("filter", &self.filter),
            ("poverty", &self.poverty),
            ("ego", &self.ego),
            ("paranoia", &self.paranoia),
            ("cultist", &self.cultist),
            ("dominance", &self.dominance),
            ("bipolar", &self.bipolar),
            ("xenophobe", &self.xenophobe),
            ("esoteric", &self.esoteric),
            ("collab", &self.collab),
        ];

        entries
            .iter()
            .filter_map(|(cat, val)| {
                val.as_deref()
                    .filter(|v| !v.is_empty() && *v != "none")
                    .map(|v| format!("{}/{}", cat, v))
            })
            .collect()
    }
}

/// Root configuration loaded from abbot.toml
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AppConfig {
    /// When true, agents are encouraged to report problems and suggest
    /// improvements to Abbot itself (developer/dogfood mode).
    #[serde(default)]
    pub developer: Option<bool>,

    /// Server configuration.
    #[serde(default)]
    pub server: ServerToml,

    /// Provider connection configuration.
    ///
    /// TOML shape:
    ///
    /// ```toml
    /// [providers.openai]
    /// base_url = "https://api.openai.com/v1"
    /// api_key_env = "OPENAI_API_KEY"
    /// ```
    #[serde(default)]
    pub providers: HashMap<String, ProviderToml>,

    /// Default LLM settings shared by head/hand/mind.
    ///
    /// TOML shape:
    ///
    /// ```toml
    /// [llm]
    /// model = "openai/gpt-4.1"
    /// temperature = 0.7
    /// max_tokens = 2048
    /// ```
    #[serde(default)]
    pub llm: LlmToml,
    /// Global trait selections (personality/behavioral directives).
    #[serde(default)]
    pub traits: TraitsToml,
    #[serde(default)]
    pub head: HeadToml,
    #[serde(default)]
    pub hand: HandToml,
    #[serde(default)]
    pub mind: MindToml,
    /// Optional config for prompt compaction/caching.
    #[serde(default)]
    pub prompt_cache: PromptCacheToml,
    #[serde(default)]
    pub harness: HarnessToml,
    #[serde(default)]
    pub vfs: VfsToml,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ServerToml {
    /// API server bind address (host:port)
    pub addr: Option<String>,
    /// Log output format: default, compact, pretty
    pub log_format: Option<String>,
    /// Upstream base URL for transparent proxy mode.
    pub proxy_base_url: Option<String>,
    /// Path to web/dist directory for the built-in UI.
    pub web_dist: Option<String>,
    /// Allow localhost peer requests to use main scope in OpenAI adapter without session markers.
    pub allow_loopback_main_scope: Option<bool>,
    /// Allow permissive CORS (`*`) for cross-origin development clients.
    pub allow_cors_any: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PromptCacheToml {
    /// Enables user system prompt compaction/caching.
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub llm: LlmToml,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct VfsToml {
    #[serde(default)]
    pub mounts: Vec<MountConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LlmToml {
    /// Model ID in "provider/model" format.
    ///
    /// OpenRouter uses: "openrouter/<upstream-provider>/<model>".
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct HeadToml {
    pub heartbeat_tick: Option<u64>,
    pub debounce_ms: Option<u64>,
    /// Insert a system time-gap marker into the head transcript when the time since
    /// the last human message exceeds this threshold (minutes). Set to 0 to disable.
    pub time_gap_marker_minutes: Option<u64>,
    /// Number of head instances in the pool (default: 3)
    pub pool: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct HandToml {
    pub max_iters: Option<usize>,
    pub max_output_chars_in_prompt: Option<usize>,
    pub max_trace_entries_in_prompt: Option<usize>,
    /// Number of hand instances in the pool (default: 4)
    pub pool: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MindToml {
    pub tick_interval: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct HarnessToml {
    /// Default model in "provider/model" format, used when `[llm].model` is not set.
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
            // Model selection is user-only; ignore any workspace-level harness model.
            model: None,
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
    pub fn init(path: impl AsRef<Path>) {
        let config = Self::load(path);
        let _ = APP_CONFIG.set(config);
    }

    /// Initialize the global config from the default path (~/.abbot/abbot.toml).
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

    /// Save config to file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let toml_str = toml::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize config: {}", e))?;
        atomic_write_file_0600(path.as_ref(), &toml_str)
            .map_err(|e| format!("failed to write config: {}", e))
    }

    /// Get a single section as JSON for API responses.
    pub fn section_json(&self, section: &str) -> Option<serde_json::Value> {
        match section {
            "server" => serde_json::to_value(&self.server).ok(),
            "providers" => serde_json::to_value(&self.providers).ok(),
            "llm" => serde_json::to_value(&self.llm).ok(),
            "traits" => serde_json::to_value(&self.traits).ok(),
            "head" => serde_json::to_value(&self.head).ok(),
            "hand" => serde_json::to_value(&self.hand).ok(),
            "mind" => serde_json::to_value(&self.mind).ok(),
            "prompt_cache" => serde_json::to_value(&self.prompt_cache).ok(),
            "harness" => serde_json::to_value(&self.harness).ok(),
            "vfs" => serde_json::to_value(&self.vfs).ok(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_toml() {
        let toml = r#"
[llm]
model = "openai/gpt-4.1"
temperature = 0.7

[head]
heartbeat_tick = 10

[hand]
max_iters = 24

[mind]
tick_interval = 60
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.llm.model, Some("openai/gpt-4.1".to_string()));
        assert_eq!(config.llm.temperature, Some(0.7));
        assert_eq!(config.head.heartbeat_tick, Some(10));
        assert_eq!(config.hand.max_iters, Some(24));
        assert_eq!(config.mind.tick_interval, Some(60));
    }

    #[test]
    fn missing_fields_are_none() {
        let toml = r#"
[llm]
model = "openai/gpt-4.1"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.llm.model, Some("openai/gpt-4.1".to_string()));
        assert_eq!(config.llm.temperature, None);
    }

    #[test]
    fn empty_config_uses_defaults() {
        let config: AppConfig = toml::from_str("").unwrap();
        assert_eq!(config.llm.model, None);
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
    fn provider_tables_parse() {
        let toml = r#"
[providers.openai]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        let p = config.providers.get("openai").unwrap();
        assert_eq!(p.base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(p.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
    }

    #[test]
    fn workspace_paths_helper() {
        let paths = WorkspacePaths::new(PathBuf::from("/home/user"));
        assert_eq!(paths.home, PathBuf::from("/home/user"));
        assert_eq!(paths.workspace, PathBuf::from("/home/user/.abbot"));
        assert_eq!(paths.mind, PathBuf::from("/home/user/.abbot/mind"));
        assert_eq!(paths.sandbox, PathBuf::from("/home/user/.abbot/sandbox"));
        assert_eq!(paths.store_db, PathBuf::from("/home/user/.abbot/store.db"));
        assert_eq!(paths.ems_db, PathBuf::from("/home/user/.abbot/ems.db"));
        assert_eq!(
            paths.frames_db,
            PathBuf::from("/home/user/.abbot/frames.db")
        );
    }

    #[test]
    fn config_serialize_roundtrip() {
        let toml = r#"
[server]
addr = "127.0.0.1:9090"
log_format = "compact"

[llm]
model = "openai/gpt-4.1"
temperature = 0.7
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let reparsed: AppConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(reparsed.server.addr, config.server.addr);
        assert_eq!(reparsed.server.log_format, config.server.log_format);
        assert_eq!(reparsed.llm.model, config.llm.model);
        assert_eq!(reparsed.llm.temperature, config.llm.temperature);
    }

    #[test]
    fn section_json_returns_sections() {
        let toml = r#"
[server]
addr = "127.0.0.1:8080"
"#;
        let config: AppConfig = toml::from_str(toml).unwrap();

        let server_json = config.section_json("server").unwrap();
        assert_eq!(server_json["addr"], serde_json::json!("127.0.0.1:8080"));

        assert!(config.section_json("unknown").is_none());
    }
}
