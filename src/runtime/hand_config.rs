#[derive(Debug, Clone)]
pub struct HandConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub max_iters: usize,
    pub max_output_chars_in_prompt: usize,
    pub max_trace_entries_in_prompt: usize,
    pub extra_headers: Vec<(String, String)>,
}

impl Default for HandConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: String::new(),
            temperature: Some(0.2),
            max_tokens: None,
            max_iters: 24,
            max_output_chars_in_prompt: 12_000,
            max_trace_entries_in_prompt: 6,
            extra_headers: Vec::new(),
        }
    }
}

impl HandConfig {
    pub fn from_env() -> Self {
        let mut cfg = HandConfig::default();

        cfg.base_url = std::env::var("HAND_BASE_URL").unwrap_or(cfg.base_url);
        cfg.api_key = std::env::var("HAND_API_KEY").unwrap_or(cfg.api_key);
        cfg.model = std::env::var("HAND_MODEL").unwrap_or(cfg.model);

        cfg.enabled = !cfg.model.trim().is_empty();

        cfg.temperature = std::env::var("HAND_TEMPERATURE")
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .or(cfg.temperature);

        cfg.max_tokens = std::env::var("HAND_MAX_TOKENS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok());

        cfg.max_iters = std::env::var("HAND_MAX_ITERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(cfg.max_iters);

        cfg.max_output_chars_in_prompt = std::env::var("HAND_MAX_OUTPUT_CHARS_IN_PROMPT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(cfg.max_output_chars_in_prompt);

        cfg.max_trace_entries_in_prompt = std::env::var("HAND_MAX_TRACE_ENTRIES_IN_PROMPT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(cfg.max_trace_entries_in_prompt);

        // Optional: "K1:V1,K2:V2"
        cfg.extra_headers = std::env::var("HAND_EXTRA_HEADERS")
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
    fn headers_csv_parses() {
        let v = parse_headers_csv("X-Test:1, X-Other: two".to_string());
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].0, "X-Test");
        assert_eq!(v[0].1, "1");
    }
}

