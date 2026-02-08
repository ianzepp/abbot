use crate::runtime::Config;
use crate::runtime::WorkspaceConfigToml;
use crate::runtime::app_config::AppConfig;

#[derive(Debug, Clone)]
pub struct RoomConfig {
    pub llm: Config,
    pub tick_interval: u64,
    pub max_rounds_conclave: usize,
    pub max_rounds_autonomy: usize,
    pub max_rounds_work: usize,
}

impl RoomConfig {
    pub fn from_config() -> Self {
        let app = AppConfig::global();
        let toml = &app.mind;

        let ws = dirs::home_dir()
            .map(|p| WorkspaceConfigToml::load_from_workspace_root(&p))
            .unwrap_or_default();

        let default_model = app.harness.model.as_deref();
        let mut llm_toml = app.llm.clone();
        llm_toml.temperature = ws.llm.temperature.or(llm_toml.temperature);
        llm_toml.max_tokens = ws.llm.max_tokens.or(llm_toml.max_tokens);
        let llm = Config::from_toml_and_env_with_default("MIND", &llm_toml, default_model);

        let tick_interval = ws.mind.tick_interval.or(toml.tick_interval).unwrap_or(60);

        Self {
            llm,
            tick_interval,
            max_rounds_conclave: 5,
            max_rounds_autonomy: 3,
            max_rounds_work: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = RoomConfig::from_config();
        assert_eq!(cfg.tick_interval, 60);
        assert_eq!(cfg.max_rounds_conclave, 5);
        assert_eq!(cfg.max_rounds_autonomy, 3);
        assert_eq!(cfg.max_rounds_work, 10);
    }
}
