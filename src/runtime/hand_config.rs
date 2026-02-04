use super::app_config::AppConfig;
use super::Config;
use super::WorkspaceConfigToml;
use super::{AutistMode, FeverMode, GenerationMode, PovertyMode, TactMode};

#[derive(Debug, Clone)]
pub struct HandConfig {
    pub llm: Config,
    pub fever: FeverMode,
    pub generation: GenerationMode,
    pub autist: AutistMode,
    pub tact: TactMode,
    pub poverty: PovertyMode,
    pub max_iters: usize,
    pub max_output_chars_in_prompt: usize,
    pub max_trace_entries_in_prompt: usize,
    pub pool_size: usize,
}

impl HandConfig {
    pub fn from_config() -> Self {
        let app = AppConfig::global();
        let toml = &app.hand;

        let ws = app
            .workspace_path()
            .ok()
            .map(|p| WorkspaceConfigToml::load_from_workspace_root(&p))
            .unwrap_or_default();

        let default_model = app.harness.model.as_deref();
        let mut llm_toml = toml.llm.clone();
        llm_toml.temperature = ws.hand.temperature.or(llm_toml.temperature);
        llm_toml.max_tokens = ws.hand.max_tokens.or(llm_toml.max_tokens);
        let llm = Config::from_toml_and_env_with_default("HAND", &llm_toml, default_model);

        let fever = ws
            .hand
            .fever
            .as_deref()
            .or(toml.fever.as_deref())
            .and_then(FeverMode::from_str)
            .unwrap_or(FeverMode::None);

        let generation = ws
            .hand
            .generation
            .as_deref()
            .or(toml.generation.as_deref())
            .and_then(GenerationMode::from_str)
            .unwrap_or(GenerationMode::None);

        let autist = ws
            .hand
            .autist
            .as_deref()
            .or(toml.autist.as_deref())
            .and_then(AutistMode::from_str)
            .unwrap_or(AutistMode::None);

        let tact = ws
            .hand
            .tact
            .as_deref()
            .or(toml.tact.as_deref())
            .and_then(TactMode::from_str)
            .unwrap_or(TactMode::None);

        let poverty = ws
            .hand
            .poverty
            .as_deref()
            .or(toml.poverty.as_deref())
            .and_then(PovertyMode::from_str)
            .unwrap_or(PovertyMode::None);

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

        let pool_size = toml.pool.or(app.pool.size).unwrap_or(4);

        Self {
            llm,
            fever,
            generation,
            autist,
            tact,
            poverty,
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
