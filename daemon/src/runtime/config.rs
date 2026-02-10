use super::app_config::{AppConfig, LlmToml};
use crate::hal::llm::LlmClient;
use crate::kernel::KernelError;

/// Common LLM configuration loaded from config.toml + provider config.
/// Each service (head, hand, mind) composes this with its own specific fields.
#[derive(Debug, Clone)]
pub struct Config {
    pub enabled: bool,
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub extra_headers: Vec<(String, String)>,
}

impl Config {
    /// Load config from config.toml + provider configuration in abbot.toml.
    ///
    /// Model IDs are always in the form:
    /// - `provider/model` (e.g., `openai/gpt-5.2`)
    /// - `openrouter/<upstream-provider>/<model>` (e.g., `openrouter/openai/gpt-5.2`)
    pub fn from_toml_and_env(_prefix: &str, toml: &LlmToml) -> Self {
        Self::from_toml_and_env_with_default("", toml, None)
    }

    /// Load config with an optional default model fallback.
    pub fn from_toml_and_env_with_default(
        _prefix: &str,
        toml: &LlmToml,
        default_model: Option<&str>,
    ) -> Self {
        let app = AppConfig::global();

        // Model ID from config or fallback.
        // (No env overrides; keep config deterministic.)
        let model_id = toml
            .model
            .clone()
            .or_else(|| default_model.map(|s| s.to_string()))
            .unwrap_or_default();

        let (provider, api_model) = parse_model_id(&model_id);

        let provider_cfg = app.providers.get(&provider);
        let base_url = provider_cfg
            .and_then(|p| p.base_url.clone())
            .unwrap_or_default();

        let api_key_env = provider_cfg
            .and_then(|p| p.api_key_env.clone())
            .unwrap_or_default();
        let api_key = if api_key_env.trim().is_empty() {
            String::new()
        } else {
            std::env::var(&api_key_env).unwrap_or_default()
        };

        let enabled = !api_model.trim().is_empty()
            && !base_url.trim().is_empty()
            && (api_key_env.trim().is_empty() || !api_key.trim().is_empty());

        let temperature = toml.temperature;
        let max_tokens = toml.max_tokens;
        let extra_headers = Vec::new();

        Self {
            enabled,
            provider,
            base_url,
            api_key,
            model: api_model,
            temperature,
            max_tokens,
            extra_headers,
        }
    }

    /// Load config for a given prefix using the global AppConfig.
    pub fn from_global(prefix: &str) -> Self {
        let app = AppConfig::global();
        match prefix {
            "HEAD" | "HAND" | "MIND" => Self::from_toml_and_env(prefix, &app.llm),
            _ => Self::from_toml_and_env(prefix, &LlmToml::default()),
        }
    }

    /// Construct an `LlmClient` from this config.
    pub fn to_llm_client(&self) -> LlmClient {
        LlmClient::new(
            &self.provider,
            &self.base_url,
            &self.api_key,
            &self.model,
            self.temperature,
            self.max_tokens,
            self.extra_headers.clone(),
        )
    }
}

/// Resolve actor-specific LLM configuration and return a ready-to-use `LlmClient`.
///
/// Actor prefixes: `head/*`, `hand/*`, `mind/*`.
pub fn client_for_actor(actor: &str) -> Result<LlmClient, KernelError> {
    use super::{HandConfig, HeadConfig, RoomConfig};

    let a = actor.trim();
    let cfg = if a.starts_with("head/") {
        HeadConfig::from_config().llm
    } else if a.starts_with("hand/") {
        HandConfig::from_config().llm
    } else if a.starts_with("mind/") {
        RoomConfig::from_config().llm
    } else {
        return Err(KernelError::invalid_args(
            "chat:llm requires actor prefix head/*, hand/*, or mind/*",
        ));
    };

    if !cfg.enabled {
        return Err(KernelError::invalid_args(format!(
            "LLM not configured for actor '{actor}'",
        )));
    }
    Ok(cfg.to_llm_client())
}

fn parse_model_id(model_id: &str) -> (String, String) {
    let model_id = model_id.trim().trim_matches('/');
    if model_id.is_empty() {
        return (String::new(), String::new());
    }
    let mut parts = model_id.split('/');
    let provider = parts.next().unwrap_or("").to_string();
    if provider.is_empty() {
        return (String::new(), model_id.to_string());
    }
    if provider == "openrouter" {
        // OpenRouter expects fully-qualified upstream ids like "openai/gpt-5.2".
        let rest: Vec<&str> = parts.collect();
        return (provider, rest.join("/"));
    }
    // Most providers want the provider-native model name, which is the trailing segment.
    (
        provider,
        model_id
            .split('/')
            .next_back()
            .unwrap_or(model_id)
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_provides_model_and_params() {
        let toml = LlmToml {
            model: Some("openai/gpt-4".to_string()),
            temperature: Some(0.5),
            max_tokens: Some(1000),
        };
        let cfg = Config::from_toml_and_env("TEST", &toml);
        // With no provider config in tests, base_url/api_key are empty and cfg is disabled.
        assert_eq!(cfg.model, "gpt-4");
        assert_eq!(cfg.base_url, "");
        assert_eq!(cfg.api_key, "");
        assert!(!cfg.enabled, "enabled requires providers.<name>.base_url");
        assert_eq!(cfg.temperature, Some(0.5));
        assert_eq!(cfg.max_tokens, Some(1000));
    }

    #[test]
    fn openrouter_model_id_preserves_upstream_path() {
        let (p, api) = parse_model_id("openrouter/openai/gpt-5.2");
        assert_eq!(p, "openrouter");
        assert_eq!(api, "openai/gpt-5.2");
    }

    #[test]
    fn to_llm_client_openai() {
        let cfg = Config {
            enabled: true,
            provider: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            model: "gpt-4".to_string(),
            temperature: Some(0.7),
            max_tokens: Some(4096),
            extra_headers: vec![],
        };
        let client = cfg.to_llm_client();
        assert!(matches!(client, crate::hal::llm::LlmClient::OpenAI(_)));
    }

    #[test]
    fn to_llm_client_anthropic() {
        let cfg = Config {
            enabled: true,
            provider: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: "sk-ant-test".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            temperature: None,
            max_tokens: Some(8192),
            extra_headers: vec![],
        };
        let client = cfg.to_llm_client();
        assert!(matches!(client, crate::hal::llm::LlmClient::Anthropic(_)));
    }

    #[test]
    fn to_llm_client_ollama_uses_openai_compat() {
        let cfg = Config {
            enabled: true,
            provider: "ollama".to_string(),
            base_url: "http://localhost:11434".to_string(),
            api_key: String::new(),
            model: "llama3".to_string(),
            temperature: None,
            max_tokens: None,
            extra_headers: vec![],
        };
        let client = cfg.to_llm_client();
        assert!(matches!(client, crate::hal::llm::LlmClient::OpenAI(_)));
    }
}
