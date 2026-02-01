use std::time::Duration;

use super::app_config::AppConfig;
use super::Config;

#[derive(Debug, Clone)]
pub struct HeadConfig {
    pub llm: Config,
    pub heartbeat_tick: u64,
    pub debounce_interval: Duration,
    pub pool_size: usize,
}

impl HeadConfig {
    pub fn from_env() -> Self {
        let app = AppConfig::global();
        let toml = &app.head;

        let default_model = app.harness.model.as_deref();
        let llm = Config::from_toml_and_env_with_default("HEAD", &toml.llm, default_model);

        let heartbeat_tick = std::env::var("HEAD_HEARTBEAT_TICK")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .or(toml.heartbeat_tick)
            .unwrap_or(60);

        let debounce_interval = std::env::var("HEAD_DEBOUNCE_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .or(toml.debounce_ms)
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(500));

        let pool_size = std::env::var("HEAD_POOL")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .or(toml.pool)
            .unwrap_or(3);

        Self {
            llm,
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
        let cfg = HeadConfig::from_env();
        assert_eq!(cfg.debounce_interval, Duration::from_millis(500));
    }
}
