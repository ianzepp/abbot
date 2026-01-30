use std::time::Duration;

use super::Config;

#[derive(Debug, Clone)]
pub struct HeadConfig {
    pub llm: Config,
    pub heartbeat_tick: u64,
    pub debounce_interval: Duration,
}

impl Default for HeadConfig {
    fn default() -> Self {
        Self {
            llm: Config::from_env("HEAD"),
            heartbeat_tick: 60,
            debounce_interval: Duration::from_millis(500),
        }
    }
}

impl HeadConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        cfg.llm.temperature = cfg.llm.temperature.or(Some(0.7));

        cfg.heartbeat_tick = std::env::var("HEAD_HEARTBEAT_TICK")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(cfg.heartbeat_tick);

        cfg.debounce_interval = std::env::var("HEAD_DEBOUNCE_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(cfg.debounce_interval);

        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HeadConfig::default();
        assert_eq!(cfg.heartbeat_tick, 60);
        assert_eq!(cfg.debounce_interval, Duration::from_millis(500));
    }
}
