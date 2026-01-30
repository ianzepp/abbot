/// Common LLM configuration that can be loaded from environment variables.
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
    /// Load config from environment variables with the given prefix.
    /// Example: `Config::from_env("HEAD")` reads `HEAD_MODEL`, `HEAD_API_KEY`, etc.
    pub fn from_env(prefix: &str) -> Self {
        let base_url = std::env::var(format!("{}_BASE_URL", prefix))
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

        let api_key = std::env::var(format!("{}_API_KEY", prefix)).unwrap_or_default();

        let model = std::env::var(format!("{}_MODEL", prefix)).unwrap_or_default();

        let enabled = !model.trim().is_empty();

        let temperature = std::env::var(format!("{}_TEMPERATURE", prefix))
            .ok()
            .and_then(|s| s.parse::<f32>().ok());

        let max_tokens = std::env::var(format!("{}_MAX_TOKENS", prefix))
            .ok()
            .and_then(|s| s.parse::<u32>().ok());

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
}
