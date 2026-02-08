//! NeedService configuration.

use crate::runtime::AppConfig;
use crate::runtime::Config;

/// Configuration for the NeedService (autonomous need processing).
pub struct NeedConfig {
    pub llm: Config,
    pub max_concurrent_rooms: usize,
    pub pool_size: usize,
}

impl NeedConfig {
    /// Build config from AppConfig global.
    pub fn from_config() -> Self {
        let config = AppConfig::global();
        let default_model = config.harness.model.as_deref();
        let llm = Config::from_toml_and_env_with_default("NEED", &config.llm, default_model);

        let max_concurrent_rooms = config.need.max_concurrent_rooms.unwrap_or(3);
        let pool_size = config.need.pool.unwrap_or(1);

        Self {
            llm,
            max_concurrent_rooms,
            pool_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = NeedConfig::from_config();
        assert_eq!(cfg.max_concurrent_rooms, 3);
        assert_eq!(cfg.pool_size, 1);
    }
}
