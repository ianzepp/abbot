use std::path::Path;

use serde::Deserialize;

use super::app_config::{read_optional_file, workspace_config_from_root};

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkspaceConfigToml {
    #[serde(default)]
    pub harness: WorkspaceHarnessToml,
    #[serde(default)]
    pub llm: WorkspaceLlmToml,
    #[serde(default)]
    pub head: WorkspaceHeadToml,
    #[serde(default)]
    pub hand: WorkspaceHandToml,
    #[serde(default)]
    pub mind: WorkspaceMindToml,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkspaceLlmToml {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkspaceHarnessToml {
    pub slow_idle: Option<u64>,
    pub deep_idle: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkspaceHeadToml {
    pub heartbeat_tick: Option<u64>,
    pub debounce_ms: Option<u64>,
    pub time_gap_marker_minutes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkspaceHandToml {
    pub max_iters: Option<usize>,
    pub max_output_chars_in_prompt: Option<usize>,
    pub max_trace_entries_in_prompt: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkspaceMindToml {
    pub tick_interval: Option<u64>,
}

impl WorkspaceConfigToml {
    pub fn load_from_workspace_root(workspace_root: &Path) -> Self {
        let config_path = workspace_config_from_root(workspace_root);
        let config_str = match read_optional_file(&config_path) {
            Ok(Some(s)) => s,
            _ => return Self::default(),
        };

        toml::from_str(&config_str).unwrap_or_default()
    }
}
