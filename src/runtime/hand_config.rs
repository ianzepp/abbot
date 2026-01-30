use super::app_config::AppConfig;
use super::Config;

#[derive(Debug, Clone)]
pub struct HandConfig {
    pub llm: Config,
    pub max_iters: usize,
    pub max_output_chars_in_prompt: usize,
    pub max_trace_entries_in_prompt: usize,
}

impl HandConfig {
    pub fn from_env() -> Self {
        let app = AppConfig::global();
        let toml = &app.hand;

        let llm = Config::from_toml_and_env("HAND", &toml.llm);

        let max_iters = std::env::var("HAND_MAX_ITERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .or(toml.max_iters)
            .unwrap_or(24);

        let max_output_chars_in_prompt = std::env::var("HAND_MAX_OUTPUT_CHARS_IN_PROMPT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .or(toml.max_output_chars_in_prompt)
            .unwrap_or(12_000);

        let max_trace_entries_in_prompt = std::env::var("HAND_MAX_TRACE_ENTRIES_IN_PROMPT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .or(toml.max_trace_entries_in_prompt)
            .unwrap_or(6);

        Self {
            llm,
            max_iters,
            max_output_chars_in_prompt,
            max_trace_entries_in_prompt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HandConfig::from_env();
        assert_eq!(cfg.max_iters, 24);
        assert_eq!(cfg.max_output_chars_in_prompt, 12_000);
        assert_eq!(cfg.max_trace_entries_in_prompt, 6);
    }
}
