use std::time::Duration;

use crate::runtime::AppConfig;
use crate::runtime::Config;
use crate::runtime::WorkspaceConfigToml;
use crate::runtime::{TarsDials, read_optional_file, workspace_config_from_root};

#[derive(Debug, Clone)]
pub struct HeadConfig {
    pub llm: Config,
    pub heartbeat_tick: u64,
    pub debounce_interval: Duration,
    pub pool_size: usize,
}

impl HeadConfig {
    pub fn from_config() -> Self {
        let app = AppConfig::global();
        let toml = &app.head;

        let ws = dirs::home_dir()
            .map(|p| WorkspaceConfigToml::load_from_workspace_root(&p))
            .unwrap_or_default();

        let default_model = app.harness.model.as_deref();
        let mut llm_toml = app.llm.clone();
        llm_toml.temperature = ws.llm.temperature.or(llm_toml.temperature);
        llm_toml.max_tokens = ws.llm.max_tokens.or(llm_toml.max_tokens);
        let llm = Config::from_toml_and_env_with_default("HEAD", &llm_toml, default_model);

        let heartbeat_tick = ws.head.heartbeat_tick.or(toml.heartbeat_tick).unwrap_or(60);

        let debounce_interval = ws
            .head
            .debounce_ms
            .or(toml.debounce_ms)
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(500));

        let pool_size = toml.pool.unwrap_or(1);

        Self {
            llm,
            heartbeat_tick,
            debounce_interval,
            pool_size,
        }
    }
}

/// Get the context budget in tokens for head bundle building.
pub(super) fn head_context_budget_tokens() -> Option<u32> {
    Some(100_000)
}

/// Get the time gap marker threshold in minutes.
pub(super) fn head_time_gap_marker_minutes() -> Option<u64> {
    let app = AppConfig::global();
    let ws = dirs::home_dir()
        .map(|p| WorkspaceConfigToml::load_from_workspace_root(&p))
        .unwrap_or_default();

    let v = ws
        .head
        .time_gap_marker_minutes
        .or(app.head.time_gap_marker_minutes)
        .unwrap_or(60);
    if v == 0 { None } else { Some(v) }
}

/// Load TARS personality dials from workspace config.
pub(super) fn load_tars_dials(workspace_root: &std::path::Path) -> TarsDials {
    let config_path = workspace_config_from_root(workspace_root);
    if !config_path.exists() {
        return TarsDials::default();
    }

    let config_str = match read_optional_file(&config_path) {
        Ok(Some(s)) => s,
        _ => return TarsDials::default(),
    };

    let config: toml::Table = match config_str.parse() {
        Ok(t) => t,
        Err(_) => return TarsDials::default(),
    };

    let Some(tars) = config.get("tars").and_then(|v| v.as_table()) else {
        return TarsDials::default();
    };

    TarsDials {
        humor: tars
            .get("humor")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        honesty: tars
            .get("honesty")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        sarcasm: tars
            .get("sarcasm")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        verbosity: tars
            .get("verbosity")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        confidence: tars
            .get("confidence")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        curiosity: tars
            .get("curiosity")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        patience: tars
            .get("patience")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        formality: tars
            .get("formality")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        empathy: tars
            .get("empathy")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        pedantry: tars
            .get("pedantry")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        initiative: tars
            .get("initiative")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        optimism: tars
            .get("optimism")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        caution: tars
            .get("caution")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HeadConfig::from_config();
        assert_eq!(cfg.debounce_interval, Duration::from_millis(500));
    }
}
