use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;

use super::models_config::{ModelDef, ModelsConfig};

static APP_CONFIG: OnceLock<AppConfig> = OnceLock::new();

/// Root configuration loaded from config.toml
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub head: HeadToml,
    #[serde(default)]
    pub hand: HandToml,
    #[serde(default)]
    pub mind: MindToml,
    #[serde(default)]
    pub pool: PoolToml,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LlmToml {
    /// Model ID in "provider/model" format (references models.toml)
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HeadToml {
    #[serde(flatten)]
    pub llm: LlmToml,
    pub heartbeat_tick: Option<u64>,
    pub debounce_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HandToml {
    #[serde(flatten)]
    pub llm: LlmToml,
    pub max_iters: Option<usize>,
    pub max_output_chars_in_prompt: Option<usize>,
    pub max_trace_entries_in_prompt: Option<usize>,
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
    /// Goal timeout in seconds (default: 300)
    pub timeout_secs: Option<u64>,
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
    /// Also initializes ModelsConfig from models.toml.
    pub fn init(path: impl AsRef<Path>) {
        let config = Self::load(path);
        let _ = APP_CONFIG.set(config);
        // Also initialize models config
        ModelsConfig::init("models.toml");
    }

    /// Get the global config. Returns default if not initialized.
    /// In production, call `init()` at startup to load from config.toml.
    /// In tests, this returns defaults (no file loading).
    pub fn global() -> &'static AppConfig {
        APP_CONFIG.get_or_init(|| {
            if cfg!(test) {
                Self::default()
            } else {
                tracing::debug!("AppConfig not initialized, loading from default path");
                Self::load("config.toml")
            }
        })
    }

    /// Look up a model definition by its full ID (e.g., "openai/gpt-4.1")
    pub fn lookup_model(&self, id: &str) -> Option<&ModelDef> {
        ModelsConfig::global().get(id)
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
}
