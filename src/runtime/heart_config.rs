use super::Config;

#[derive(Debug, Clone)]
pub struct HeartConfig {
    pub llm: Config,
    pub tick_interval: u64,
}

impl Default for HeartConfig {
    fn default() -> Self {
        Self {
            llm: Config::from_env("HEART"),
            tick_interval: 60,
        }
    }
}

impl HeartConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        cfg.llm.temperature = cfg.llm.temperature.or(Some(0.7));

        cfg.tick_interval = std::env::var("HEART_TICK")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(cfg.tick_interval);

        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HeartConfig::default();
        assert_eq!(cfg.tick_interval, 60);
    }
}
