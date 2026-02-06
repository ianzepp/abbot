//! Room Configuration - Runtime settings for room deliberation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Loads room configuration from the global AppConfig and workspace-level TOML
//! overrides. Configuration controls LLM provider settings, behavioral modes
//! (fever, filter, poverty), tick intervals, and per-room-type round limits.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Layered config: Global `abbot.toml` provides defaults, workspace-level
//!   `.abbot/config.toml` overrides per-workspace. Environment variables
//!   (`MIND_*`) override both.
//! - Replaces MindConfig: Same fields and loading logic, extended with
//!   per-room-type round limits (conclave=5, autonomy=3, work=10).
//!
//! TRADE-OFFS
//! ==========
//! - Round limits are hardcoded defaults rather than configurable via TOML.
//!   Acceptable because wrong values cause either wasted LLM calls (too high)
//!   or premature termination (too low), and the defaults are well-tested.

use crate::runtime::app_config::AppConfig;
use crate::runtime::Config;
use crate::runtime::WorkspaceConfigToml;
use crate::runtime::{AutistMode, FeverMode, FilterMode, GenerationMode, PovertyMode};

/// Runtime configuration for room deliberation.
///
/// WHY this exists: Centralizes all settings that affect room behavior so that
/// the runner, coordinator, and bundle builder share a single config source.
#[derive(Debug, Clone)]
pub struct RoomConfig {
    /// LLM provider settings (model, temperature, max_tokens, API key).
    pub llm: Config,
    /// WHY fever: Controls how aggressively the coordinator schedules rooms.
    /// None = normal idle-based, Meth = immediate on first idle.
    pub fever: FeverMode,
    pub generation: GenerationMode,
    pub autist: AutistMode,
    pub filter: FilterMode,
    pub poverty: PovertyMode,
    /// WHY tick_interval: Determines how often the coordinator checks for
    /// idle state. Lower values increase responsiveness but waste CPU.
    pub tick_interval: u64,
    pub max_rounds_conclave: usize,
    pub max_rounds_autonomy: usize,
    pub max_rounds_work: usize,
}

impl RoomConfig {
    /// Load configuration from AppConfig + workspace TOML.
    ///
    /// WHY this pattern: Mirrors HeadConfig::from_config() and
    /// HandConfig::from_config() for consistency across agent types.
    pub fn from_config() -> Self {
        let app = AppConfig::global();
        let toml = &app.mind;

        let ws = app
            .workspace_path()
            .ok()
            .map(|p| WorkspaceConfigToml::load_from_workspace_root(&p))
            .unwrap_or_default();

        // WHY layered: workspace TOML overrides global TOML for temperature
        // and max_tokens, enabling per-project tuning without changing global config.
        let default_model = app.harness.model.as_deref();
        let mut llm_toml = toml.llm.clone();
        llm_toml.temperature = ws.mind.temperature.or(llm_toml.temperature);
        llm_toml.max_tokens = ws.mind.max_tokens.or(llm_toml.max_tokens);
        let llm = Config::from_toml_and_env_with_default("MIND", &llm_toml, default_model);

        let fever = ws
            .mind
            .fever
            .as_deref()
            .or(toml.fever.as_deref())
            .and_then(FeverMode::from_str)
            .unwrap_or(FeverMode::None);

        let generation = ws
            .mind
            .generation
            .as_deref()
            .or(toml.generation.as_deref())
            .and_then(GenerationMode::from_str)
            .unwrap_or(GenerationMode::None);

        let autist = ws
            .mind
            .autist
            .as_deref()
            .or(toml.autist.as_deref())
            .and_then(AutistMode::from_str)
            .unwrap_or(AutistMode::None);

        let filter = ws
            .mind
            .filter
            .as_deref()
            .or(toml.filter.as_deref())
            .and_then(FilterMode::from_str)
            .unwrap_or(FilterMode::None);

        let poverty = ws
            .mind
            .poverty
            .as_deref()
            .or(toml.poverty.as_deref())
            .and_then(PovertyMode::from_str)
            .unwrap_or(PovertyMode::None);

        let tick_interval = ws.mind.tick_interval.or(toml.tick_interval).unwrap_or(60);

        Self {
            llm,
            fever,
            generation,
            autist,
            filter,
            poverty,
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
