use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HeadConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub heartbeat_tick: u64,
    pub debounce_interval: Duration,
    pub extra_headers: Vec<(String, String)>,
}

impl Default for HeadConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: String::new(),
            temperature: Some(0.7),
            max_tokens: None,
            heartbeat_tick: 60,
            debounce_interval: Duration::from_millis(500),
            extra_headers: Vec::new(),
        }
    }
}

impl HeadConfig {
    pub fn from_env() -> Self {
        let mut cfg = HeadConfig::default();

        cfg.base_url = std::env::var("HEAD_BASE_URL").unwrap_or(cfg.base_url);
        cfg.api_key = std::env::var("HEAD_API_KEY").unwrap_or(cfg.api_key);
        cfg.model = std::env::var("HEAD_MODEL").unwrap_or(cfg.model);

        cfg.enabled = !cfg.model.trim().is_empty();

        cfg.temperature = std::env::var("HEAD_TEMPERATURE")
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .or(cfg.temperature);

        cfg.max_tokens = std::env::var("HEAD_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok());

        // Every N ticks (1 tick = 1 second). 0 = disabled.
        cfg.heartbeat_tick = std::env::var("HEAD_HEARTBEAT_TICK")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(cfg.heartbeat_tick);

        cfg.debounce_interval = std::env::var("HEAD_DEBOUNCE_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(cfg.debounce_interval);

        cfg.extra_headers = std::env::var("HEAD_EXTRA_HEADERS")
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
        let cfg = HeadConfig::default();
        assert_eq!(cfg.heartbeat_tick, 60);
        assert_eq!(cfg.debounce_interval, Duration::from_millis(500));
    }
}
