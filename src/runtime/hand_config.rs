use super::Config;

#[derive(Debug, Clone)]
pub struct HandConfig {
    pub llm: Config,
    pub max_iters: usize,
    pub max_output_chars_in_prompt: usize,
    pub max_trace_entries_in_prompt: usize,
}

impl Default for HandConfig {
    fn default() -> Self {
        Self {
            llm: Config::from_env("HAND"),
            max_iters: 24,
            max_output_chars_in_prompt: 12_000,
            max_trace_entries_in_prompt: 6,
        }
    }
}

impl HandConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        cfg.llm.temperature = cfg.llm.temperature.or(Some(0.2));

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

        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HandConfig::default();
        assert_eq!(cfg.max_iters, 24);
        assert_eq!(cfg.max_output_chars_in_prompt, 12_000);
        assert_eq!(cfg.max_trace_entries_in_prompt, 6);
    }
}
