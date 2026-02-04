use super::app_config::AppConfig;
use super::Config;

#[derive(Debug, Clone)]
pub struct MindConfig {
    pub llm: Config,
    pub tick_interval: u64,
}

impl MindConfig {
    pub fn from_config() -> Self {
        let app = AppConfig::global();
        let toml = &app.mind;

        let default_model = app.harness.model.as_deref();
        let llm = Config::from_toml_and_env_with_default("MIND", &toml.llm, default_model);

        let tick_interval = toml.tick_interval.unwrap_or(60);

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
