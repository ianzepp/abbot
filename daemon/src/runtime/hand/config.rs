use crate::runtime::{AppConfig, Config, WorkspaceConfigToml};

#[derive(Debug, Clone)]
pub struct HandConfig {
    pub llm: Config,
    pub traits: Vec<String>,
    pub max_iters: usize,
    pub max_output_chars_in_prompt: usize,
    pub max_trace_entries_in_prompt: usize,
    pub pool_size: usize,
}

impl HandConfig {
    pub fn from_config() -> Self {
        let app = AppConfig::global();
        let toml = &app.hand;

        let ws = dirs::home_dir()
            .map(|p| WorkspaceConfigToml::load_from_workspace_root(&p))
            .unwrap_or_default();

        let default_model = app.harness.model.as_deref();
        let mut llm_toml = app.llm.clone();
        llm_toml.temperature = ws.llm.temperature.or(llm_toml.temperature);
        llm_toml.max_tokens = ws.llm.max_tokens.or(llm_toml.max_tokens);
        let llm = Config::from_toml_and_env_with_default("HAND", &llm_toml, default_model);

        let traits = if !ws.hand.traits.is_empty() {
            ws.hand.traits.clone()
        } else if !toml.traits.is_empty() {
            toml.traits.clone()
        } else {
            app.traits.to_trait_names()
        };

        let max_iters = ws.hand.max_iters.or(toml.max_iters).unwrap_or(24);

        let max_output_chars_in_prompt = ws
            .hand
            .max_output_chars_in_prompt
            .or(toml.max_output_chars_in_prompt)
            .unwrap_or(12_000);

        let max_trace_entries_in_prompt = ws
            .hand
            .max_trace_entries_in_prompt
            .or(toml.max_trace_entries_in_prompt)
            .unwrap_or(6);

        let pool_size = toml.pool.unwrap_or(4);

        Self {
            llm,
            traits,
            max_iters,
            max_output_chars_in_prompt,
            max_trace_entries_in_prompt,
            pool_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HandConfig::from_config();
        assert_eq!(cfg.max_iters, 24);
        assert_eq!(cfg.max_output_chars_in_prompt, 12_000);
        assert_eq!(cfg.max_trace_entries_in_prompt, 6);
    }
}
