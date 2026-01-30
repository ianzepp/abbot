use super::app_config::{AppConfig, LlmToml};

/// Common LLM configuration loaded from config.toml + env vars.
/// Each service (head, hand, heart) composes this with its own specific fields.
#[derive(Debug, Clone)]
pub struct Config {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub extra_headers: Vec<(String, String)>,
}

impl Config {
    /// Load config from config.toml + environment variables.
    /// TOML provides defaults, env vars override.
    ///
    /// API key resolution order:
    /// 1. `{PREFIX}_API_KEY` env var (e.g., HEAD_API_KEY)
    /// 2. Env var named in `toml.api_key` (e.g., if api_key = "OPENAI_API_KEY", read $OPENAI_API_KEY)
    pub fn from_toml_and_env(prefix: &str, toml: &LlmToml) -> Self {
        let base_url = std::env::var(format!("{}_BASE_URL", prefix))
            .ok()
            .or_else(|| toml.base_url.clone())
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let api_key = std::env::var(format!("{}_API_KEY", prefix))
            .ok()
            .or_else(|| {
                toml.api_key
                    .as_ref()
                    .and_then(|var_name| std::env::var(var_name).ok())
            })
            .unwrap_or_default();

        let model = std::env::var(format!("{}_MODEL", prefix))
            .ok()
            .or_else(|| toml.model.clone())
            .unwrap_or_default();

        let enabled = !model.trim().is_empty() && !api_key.trim().is_empty();

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
            "HEART" => &app.heart.llm,
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
    fn toml_provides_defaults() {
        let toml = LlmToml {
            model: Some("gpt-4".to_string()),
            base_url: Some("https://custom.api".to_string()),
            api_key: None,
            temperature: Some(0.5),
            max_tokens: Some(1000),
        };
        let cfg = Config::from_toml_and_env("TEST", &toml);
        assert_eq!(cfg.model, "gpt-4");
        assert_eq!(cfg.base_url, "https://custom.api");
        assert_eq!(cfg.temperature, Some(0.5));
        assert_eq!(cfg.max_tokens, Some(1000));
        assert!(!cfg.enabled, "enabled requires both model and api_key");
    }

    #[test]
    fn api_key_from_env_var_reference() {
        unsafe { std::env::set_var("TEST_PROVIDER_KEY", "sk-test-key") };
        let toml = LlmToml {
            model: Some("gpt-4".to_string()),
            base_url: None,
            api_key: Some("TEST_PROVIDER_KEY".to_string()),
            temperature: None,
            max_tokens: None,
        };
        let cfg = Config::from_toml_and_env("TEST_REF", &toml);
        assert_eq!(cfg.api_key, "sk-test-key");
        assert!(cfg.enabled);
        unsafe { std::env::remove_var("TEST_PROVIDER_KEY") };
    }
}
