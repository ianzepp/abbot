//! Mind Loop Configuration
//!
//! Runtime settings for the proactive mind loop observer. Loaded from the
//! existing `[mind]` TOML section with hardcoded defaults.

use crate::runtime::app_config::AppConfig;

/// Runtime configuration for the mind loop.
#[derive(Debug, Clone)]
pub struct MindLoopConfig {
    /// Seconds between wake cycles (default: 60 = 1 minute).
    pub cadence_secs: u64,
    /// Maximum LLM rounds per wake cycle (safety cap).
    pub max_rounds: usize,
    /// Channel to observe (default: "main").
    pub channel: String,
    /// Maximum context items to include per section.
    pub max_context_items: usize,
    /// Milliseconds to sleep between tool-use rounds (prevents API saturation).
    pub round_delay_ms: u64,
}

impl MindLoopConfig {
    /// Load configuration from AppConfig.
    ///
    /// Reads `tick_interval` from the existing `[mind]` TOML section for
    /// cadence. Other fields use hardcoded defaults.
    pub fn from_config() -> Self {
        let app = AppConfig::global();
        let cadence_secs = app.mind.tick_interval.unwrap_or(60);

        Self {
            cadence_secs,
            max_rounds: 8,
            channel: "main".to_string(),
            max_context_items: 100,
            round_delay_ms: 500,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = MindLoopConfig::from_config();
        assert_eq!(cfg.cadence_secs, 60);
        assert_eq!(cfg.max_rounds, 8);
        assert_eq!(cfg.channel, "main");
        assert_eq!(cfg.max_context_items, 100);
        assert_eq!(cfg.round_delay_ms, 500);
    }
}
