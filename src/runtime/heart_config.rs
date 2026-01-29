#[derive(Debug, Clone)]
pub struct HeartConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tick_interval: u64,
    pub extra_headers: Vec<(String, String)>,
}

impl Default for HeartConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: String::new(),
            temperature: Some(0.7),
            max_tokens: None,
            tick_interval: 60,
            extra_headers: Vec::new(),
        }
    }
}

impl HeartConfig {
    pub fn from_env() -> Self {
        let mut cfg = HeartConfig::default();

        cfg.base_url = std::env::var("HEART_BASE_URL").unwrap_or(cfg.base_url);
        cfg.api_key = std::env::var("HEART_API_KEY").unwrap_or(cfg.api_key);
        cfg.model = std::env::var("HEART_MODEL").unwrap_or(cfg.model);

        cfg.enabled = !cfg.model.trim().is_empty();

        cfg.temperature = std::env::var("HEART_TEMPERATURE")
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .or(cfg.temperature);

        cfg.max_tokens = std::env::var("HEART_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok());

        // Run every N ticks (1 tick = 1 second). Default 60.
        cfg.tick_interval = std::env::var("HEART_TICK")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(cfg.tick_interval);

        cfg.extra_headers = std::env::var("HEART_EXTRA_HEADERS")
            .ok()
            .map(parse_headers_csv)
            .unwrap_or_default();

        cfg
    }
}

fn parse_headers_csv(s: String) -> Vec<(String, String)> {
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
    fn default_config() {
        let cfg = HeartConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.tick_interval, 60);
        assert_eq!(cfg.temperature, Some(0.7));
    }
}
