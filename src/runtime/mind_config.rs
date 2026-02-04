use super::app_config::AppConfig;
use super::Config;
use super::WorkspaceConfigToml;

#[derive(Debug, Clone)]
pub struct MindConfig {
    pub llm: Config,
    pub tick_interval: u64,
}

impl MindConfig {
    pub fn from_config() -> Self {
        let app = AppConfig::global();
        let toml = &app.mind;

        let ws = app
            .workspace_path()
            .ok()
            .map(|p| WorkspaceConfigToml::load_from_workspace_root(&p))
            .unwrap_or_default();

        let default_model = app.harness.model.as_deref();
        let mut llm_toml = toml.llm.clone();
        llm_toml.temperature = ws.mind.temperature.or(llm_toml.temperature);
        llm_toml.max_tokens = ws.mind.max_tokens.or(llm_toml.max_tokens);
        let llm = Config::from_toml_and_env_with_default("MIND", &llm_toml, default_model);

        let tick_interval = ws.mind.tick_interval.or(toml.tick_interval).unwrap_or(60);

        Self { llm, tick_interval }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = MindConfig::from_config();
        assert_eq!(cfg.tick_interval, 60);
    }
}
