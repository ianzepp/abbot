use super::app_config::AppConfig;
use super::Config;

#[derive(Debug, Clone)]
pub struct HeartConfig {
    pub llm: Config,
    pub tick_interval: u64,
}

impl HeartConfig {
    pub fn from_env() -> Self {
        let app = AppConfig::global();
        let toml = &app.heart;

        let llm = Config::from_toml_and_env("HEART", &toml.llm);

        let tick_interval = std::env::var("HEART_TICK")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .or(toml.tick_interval)
            .unwrap_or(60);

        Self { llm, tick_interval }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HeartConfig::from_env();
        assert_eq!(cfg.tick_interval, 60);
    }
}
