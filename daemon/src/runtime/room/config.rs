//! Room Config - LLM and execution parameters for room sessions
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Loads room execution configuration from AppConfig and workspace TOML. The room
//! config drives LLM selection (model, temperature, max_tokens) and the default
//! round limit for room execution.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Single max_rounds**: All room types share a default round limit (10). Callers
//!   can override per-invocation via the `room:run` syscall's `max_rounds` parameter.
//! - **LLM config inheritance**: Room LLM config inherits from the global MIND config
//!   with workspace TOML overrides, since rooms are a form of reflective execution.

use crate::runtime::Config;
use crate::runtime::WorkspaceConfigToml;
use crate::runtime::app_config::AppConfig;

// =============================================================================
// CONFIGURATION
// =============================================================================

/// Room execution configuration: LLM settings and round limits.
///
/// WHY: Centralizes room execution parameters so both `room:run` and `RoomRunner`
/// use consistent defaults. The LLM config determines which model handles agent
/// turns and summarization.
#[derive(Debug, Clone)]
pub struct RoomConfig {
    pub llm: Config,
    /// Default maximum rounds before forced termination.
    pub max_rounds: usize,
}

impl RoomConfig {
    /// Load room config from AppConfig + workspace TOML.
    ///
    /// WHY inherits from MIND config: Rooms are reflective execution contexts
    /// (like the mind loop), so they share the same model selection and parameter
    /// defaults. Workspace TOML overrides allow per-project tuning.
    pub fn from_config() -> Self {
        let app = AppConfig::global();

        let ws = dirs::home_dir()
            .map(|p| WorkspaceConfigToml::load_from_workspace_root(&p))
            .unwrap_or_default();

        let default_model = app.harness.model.as_deref();
        let mut llm_toml = app.llm.clone();
        llm_toml.temperature = ws.llm.temperature.or(llm_toml.temperature);
        llm_toml.max_tokens = ws.llm.max_tokens.or(llm_toml.max_tokens);
        let llm = Config::from_toml_and_env_with_default("MIND", &llm_toml, default_model);

        Self {
            llm,
            max_rounds: 10,
        }
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = RoomConfig::from_config();
        assert_eq!(cfg.max_rounds, 10);
    }
}
