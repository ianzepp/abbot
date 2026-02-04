use std::time::Duration;

use super::app_config::AppConfig;
use super::Config;
use super::WorkspaceConfigToml;
use super::{AutistMode, FeverMode, GenerationMode, TactMode};

#[derive(Debug, Clone)]
pub struct HeadConfig {
    pub llm: Config,
    pub fever: FeverMode,
    pub generation: GenerationMode,
    pub autist: AutistMode,
    pub tact: TactMode,
    pub heartbeat_tick: u64,
    pub debounce_interval: Duration,
    pub pool_size: usize,
}

impl HeadConfig {
    pub fn from_config() -> Self {
        let app = AppConfig::global();
        let toml = &app.head;

        let ws = app
            .workspace_path()
            .ok()
            .map(|p| WorkspaceConfigToml::load_from_workspace_root(&p))
            .unwrap_or_default();

        let default_model = app.harness.model.as_deref();
        let mut llm_toml = toml.llm.clone();
        llm_toml.temperature = ws.head.temperature.or(llm_toml.temperature);
        llm_toml.max_tokens = ws.head.max_tokens.or(llm_toml.max_tokens);
        let llm = Config::from_toml_and_env_with_default("HEAD", &llm_toml, default_model);

        let fever = ws
            .head
            .fever
            .as_deref()
            .or(toml.fever.as_deref())
            .and_then(FeverMode::from_str)
            .unwrap_or(FeverMode::None);

        let generation = ws
            .head
            .generation
            .as_deref()
            .or(toml.generation.as_deref())
            .and_then(GenerationMode::from_str)
            .unwrap_or(GenerationMode::None);

        let autist = ws
            .head
            .autist
            .as_deref()
            .or(toml.autist.as_deref())
            .and_then(AutistMode::from_str)
            .unwrap_or(AutistMode::None);

        let tact = ws
            .head
            .tact
            .as_deref()
            .or(toml.tact.as_deref())
            .and_then(TactMode::from_str)
            .unwrap_or(TactMode::None);

        let heartbeat_tick = ws.head.heartbeat_tick.or(toml.heartbeat_tick).unwrap_or(60);

        let debounce_interval = ws
            .head
            .debounce_ms
            .or(toml.debounce_ms)
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(500));

        let pool_size = toml.pool.unwrap_or(3);

        Self {
            llm,
            fever,
            generation,
            autist,
            tact,
            heartbeat_tick,
            debounce_interval,
            pool_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HeadConfig::from_config();
        assert_eq!(cfg.debounce_interval, Duration::from_millis(500));
    }
}
