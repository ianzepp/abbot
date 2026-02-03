use super::app_config::{AppConfig, LlmToml};

/// Common LLM configuration loaded from config.toml + models.toml + env vars.
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

/// Resolved model configuration from models.toml.
/// Returned when looking up a model by ID.
pub struct ResolvedModel {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub api_model: String,
}

impl Config {
    /// Resolve model configuration by looking up model_id in models.toml.
    /// Returns None if model not found.
    fn resolve_model(model_id: &str) -> Option<ResolvedModel> {
        let models = super::models_config::ModelsConfig::global();
        let model_def = models.get(model_id)?;

        Some(ResolvedModel {
            provider: model_def.provider.clone(),
            base_url: model_def.base_url.clone(),
            api_key: model_def.api_key(),
            api_model: api_model_name(&model_def.id),
        })
    }

    /// Load config from config.toml + models.toml + environment variables.
    /// Resolution order for model/base_url/api_key:
    /// 1. `{PREFIX}_MODEL` env var (e.g., HEAD_MODEL) - overrides model ID
    /// 2. `toml.model` from config.toml - references models.toml entry
    /// 3. `default_model` fallback (e.g., harness.model)
    ///
    /// Then lookup in models.toml to get base_url and api_key.
    ///
    /// Override precedence for base_url/api_key:
    /// - `{PREFIX}_BASE_URL` / `{PREFIX}_API_KEY` env vars override models.toml values
    pub fn from_toml_and_env(prefix: &str, toml: &LlmToml) -> Self {
        Self::from_toml_and_env_with_default(prefix, toml, None)
    }

    /// Load config with an optional default model fallback.
    pub fn from_toml_and_env_with_default(
        prefix: &str,
        toml: &LlmToml,
        default_model: Option<&str>,
    ) -> Self {
        let app = AppConfig::global();
        let app_model = app.model.as_ref();

        // Get model ID from env, config, or default
        let model_id = std::env::var(format!("{}_MODEL", prefix))
            .ok()
            .or_else(|| toml.model.clone())
            .or_else(|| default_model.map(|s| s.to_string()))
            .unwrap_or_default();

        // Look up model in models.toml (legacy path)
        let resolved = Self::resolve_model(&model_id);

        let provider = std::env::var(format!("{}_PROVIDER", prefix))
            .ok()
            .or_else(|| toml.provider.clone())
            .or_else(|| app_model.and_then(|m| m.provider.clone()))
            .or_else(|| resolved.as_ref().map(|r| r.provider.clone()))
            .unwrap_or_else(|| "openai".to_string());

        let base_url = std::env::var(format!("{}_BASE_URL", prefix))
            .ok()
            .or_else(|| toml.base_url.clone())
            .or_else(|| app_model.and_then(|m| m.base_url.clone()))
            .or_else(|| resolved.as_ref().map(|r| r.base_url.clone()))
            .unwrap_or_default();

        let api_key = std::env::var(format!("{}_API_KEY", prefix))
            .ok()
            .or_else(|| toml.api_key.clone())
            .or_else(|| app_model.and_then(|m| m.api_key.clone()))
            .or_else(|| {
                let env_key = toml
                    .api_key_env
                    .clone()
                    .or_else(|| app_model.and_then(|m| m.api_key_env.clone()));
                env_key.and_then(|k| std::env::var(k).ok())
            })
            .or_else(|| resolved.as_ref().map(|r| r.api_key.clone()))
            .unwrap_or_default();

        let model = resolved
            .as_ref()
            .map(|r| r.api_model.clone())
            .unwrap_or_else(|| model_id.clone());

        let enabled = !model.trim().is_empty() && !base_url.trim().is_empty();

        let temperature = std::env::var(format!("{}_TEMPERATURE", prefix))
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .or(toml.temperature);

        let max_tokens = std::env::var(format!("{}_MAX_TOKENS", prefix))
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .or(toml.max_tokens);

        let extra_headers = std::env::var(format!("{}_EXTRA_HEADERS", prefix))
            .ok()
            .map(|s| parse_headers_csv(&s))
            .unwrap_or_default();

        Self {
            enabled,
            provider,
            base_url,
            api_key,
            model,
            temperature,
            max_tokens,
            extra_headers,
        }
    }

    /// Load config for a given prefix using the global AppConfig.
    pub fn from_global(prefix: &str) -> Self {
        let app = AppConfig::global();
        let toml = match prefix {
            "HEAD" => &app.head.llm,
            "HAND" => &app.hand.llm,
            "MIND" => &app.mind.llm,
            _ => return Self::from_toml_and_env(prefix, &LlmToml::default()),
        };
        Self::from_toml_and_env(prefix, toml)
    }
}

fn parse_headers_csv(s: &str) -> Vec<(String, String)> {
    s.split(',')
        .filter_map(|pair| {
            let pair = pair.trim();
            if pair.is_empty() {
                return None;
            }
            let (k, v) = pair.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

fn api_model_name(id: &str) -> String {
    // Model IDs in config.toml/models.toml use "provider/model" so we can share one namespace.
    // API payloads typically expect the provider-specific model name (the trailing segment).
    id.split('/').last().unwrap_or(id).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headers_csv() {
        let v = parse_headers_csv("X-Test:1, X-Other: two");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].0, "X-Test");
        assert_eq!(v[0].1, "1");
        assert_eq!(v[1].0, "X-Other");
        assert_eq!(v[1].1, "two");
    }

    #[test]
    fn empty_headers() {
        let v = parse_headers_csv("");
        assert!(v.is_empty());
    }

    #[test]
    fn toml_provides_model_and_params() {
        // Note: model resolution requires models.toml - without it,
        // model ID is passed through but base_url/api_key are empty
        let toml = LlmToml {
            model: Some("openai/gpt-4".to_string()),
            temperature: Some(0.5),
            max_tokens: Some(1000),
        };
        let cfg = Config::from_toml_and_env("TEST", &toml);
        // Without models.toml entry, model ID passes through
        assert_eq!(cfg.model, "openai/gpt-4");
        // Without models.toml, base_url and api_key are empty
        assert_eq!(cfg.base_url, "");
        assert_eq!(cfg.api_key, "");
        assert!(!cfg.enabled, "enabled requires base_url from models.toml");
        assert_eq!(cfg.temperature, Some(0.5));
        assert_eq!(cfg.max_tokens, Some(1000));
    }

    #[test]
    fn env_vars_override_toml() {
        unsafe {
            std::env::set_var("TESTOVERRIDE_MODEL", "custom/model");
            std::env::set_var("TESTOVERRIDE_BASE_URL", "https://override.com");
            std::env::set_var("TESTOVERRIDE_API_KEY", "sk-override");
            std::env::set_var("TESTOVERRIDE_TEMPERATURE", "0.9");
        }
        let toml = LlmToml {
            model: Some("openai/gpt-4".to_string()),
            temperature: Some(0.5),
            max_tokens: None,
        };
        let cfg = Config::from_toml_and_env("TESTOVERRIDE", &toml);
        assert_eq!(cfg.model, "custom/model");
        assert_eq!(cfg.base_url, "https://override.com");
        assert_eq!(cfg.api_key, "sk-override");
        assert_eq!(cfg.temperature, Some(0.9));
        assert!(cfg.enabled);
        unsafe {
            std::env::remove_var("TESTOVERRIDE_MODEL");
            std::env::remove_var("TESTOVERRIDE_BASE_URL");
            std::env::remove_var("TESTOVERRIDE_API_KEY");
            std::env::remove_var("TESTOVERRIDE_TEMPERATURE");
        }
    }

    #[test]
    fn api_model_name_strips_provider_prefix() {
        assert_eq!(api_model_name("openai/gpt-4.1"), "gpt-4.1");
        assert_eq!(api_model_name("ollama/llama3.2"), "llama3.2");
        assert_eq!(api_model_name("gpt-4.1"), "gpt-4.1");
        assert_eq!(api_model_name("custom/provider/model"), "model");
    }
}
