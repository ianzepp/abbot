use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;

static MODELS_CONFIG: OnceLock<ModelsConfig> = OnceLock::new();

/// A model definition from models.toml
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ModelDef {
    pub id: String,
    pub provider: String,
    pub base_url: String,
    /// Name of env var containing the API key (e.g., "OPENAI_API_KEY")
    pub api_key_env: String,
    pub context_window: Option<u32>,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub supports_vision: bool,
}

impl ModelDef {
    /// Get the API key from the environment variable specified in api_key_env.
    /// Returns empty string if api_key_env is empty or env var is not set.
    pub fn api_key(&self) -> String {
        if self.api_key_env.is_empty() {
            return String::new();
        }
        std::env::var(&self.api_key_env).unwrap_or_default()
    }
}

/// Root configuration loaded from models.toml
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ModelsConfig {
    #[serde(rename = "model", default)]
    pub models: Vec<ModelDef>,
}

impl ModelsConfig {
    /// Load models config from file, or return empty if file doesn't exist.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        if !path.exists() {
            tracing::debug!(?path, "models.toml not found, using empty config");
            return Self::default();
        }

        match std::fs::read_to_string(path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => {
                    tracing::info!(?path, "loaded models config");
                    config
                }
                Err(e) => {
                    tracing::warn!(?path, error = %e, "failed to parse models.toml, using empty");
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!(?path, error = %e, "failed to read models.toml, using empty");
                Self::default()
            }
        }
    }

    /// Initialize the global models config. Call once at startup.
    pub fn init(path: impl AsRef<Path>) {
        let config = Self::load(path);
        let _ = MODELS_CONFIG.set(config);
    }

    /// Get the global models config. Returns empty if not initialized.
    pub fn global() -> &'static Self {
        MODELS_CONFIG.get_or_init(|| {
            if cfg!(test) {
                Self::default()
            } else {
                tracing::debug!("ModelsConfig not initialized, loading from default path");
                Self::load("models.toml")
            }
        })
    }

    /// Look up a model by its full ID (e.g., "openai/gpt-4.1")
    pub fn get(&self, id: &str) -> Option<&ModelDef> {
        self.models.iter().find(|m| m.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_models_toml() {
        let toml = r#"
[[model]]
id = "openai/gpt-4.1"
provider = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
context_window = 128000
supports_tools = true
supports_vision = true

[[model]]
id = "ollama/llama3.2"
provider = "ollama"
base_url = "http://localhost:11434/v1"
api_key_env = ""
context_window = 128000
supports_tools = false
supports_vision = false
"#;
        let config: ModelsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.models.len(), 2);

        let openai = config.get("openai/gpt-4.1").unwrap();
        assert_eq!(openai.provider, "openai");
        assert_eq!(openai.base_url, "https://api.openai.com/v1");
        assert_eq!(openai.api_key_env, "OPENAI_API_KEY");
        assert_eq!(openai.context_window, Some(128000));
        assert!(openai.supports_tools);
        assert!(openai.supports_vision);

        let ollama = config.get("ollama/llama3.2").unwrap();
        assert_eq!(ollama.provider, "ollama");
        assert_eq!(ollama.base_url, "http://localhost:11434/v1");
        assert_eq!(ollama.api_key_env, "");
        assert!(!ollama.supports_tools);
        assert!(!ollama.supports_vision);
    }

    #[test]
    fn model_lookup_not_found() {
        let config = ModelsConfig::default();
        assert!(config.get("nonexistent/model").is_none());
    }

    #[test]
    fn api_key_from_env() {
        unsafe { std::env::set_var("TEST_API_KEY", "sk-test123") };
        let model = ModelDef {
            id: "test/model".to_string(),
            provider: "test".to_string(),
            base_url: "https://test.com".to_string(),
            api_key_env: "TEST_API_KEY".to_string(),
            context_window: None,
            supports_tools: false,
            supports_vision: false,
        };
        assert_eq!(model.api_key(), "sk-test123");
        unsafe { std::env::remove_var("TEST_API_KEY") };
    }

    #[test]
    fn api_key_empty_when_no_env_var() {
        let model = ModelDef {
            id: "local/model".to_string(),
            provider: "local".to_string(),
            base_url: "http://localhost".to_string(),
            api_key_env: "".to_string(),
            context_window: None,
            supports_tools: false,
            supports_vision: false,
        };
        assert_eq!(model.api_key(), "");
    }
}
