//! Config - Path resolution, API key management, provider cache
//!
//! Shared configuration helpers used by CLI commands that work offline
//! (reading config files, managing API keys, caching provider models).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use abbot::runtime::app_config::atomic_write_file_0600;

// =============================================================================
// PATH RESOLUTION
// =============================================================================

pub fn config_dir() -> Option<PathBuf> {
    abbot::runtime::app_config::config_dir()
}

pub fn default_config_path() -> Option<PathBuf> {
    abbot::runtime::app_config::default_config_path()
}

pub fn keys_path() -> Option<PathBuf> {
    abbot::runtime::app_config::keys_path()
}

pub fn providers_dir() -> Option<PathBuf> {
    abbot::runtime::app_config::providers_dir()
}

/// Resolve the config file path from `--config` or default (~/.abbot/abbot.toml).
pub fn resolve_config_path(cli_config: Option<&Path>) -> Option<PathBuf> {
    cli_config
        .map(|p| p.to_path_buf())
        .or_else(default_config_path)
}

/// Derive the default `rpc.sock` path from the data directory.
pub fn default_rpc_sock(_cli_config: Option<&Path>) -> Option<PathBuf> {
    config_dir().map(|d| d.join("rpc.sock"))
}

// =============================================================================
// API KEY MANAGEMENT
// =============================================================================

/// Load API keys from ~/.abbot/keys.env and set as environment variables.
/// Returns the keys that were loaded (for display purposes).
pub fn load_api_keys() -> Vec<(String, String)> {
    let path = match keys_path() {
        Some(p) => p,
        None => return Vec::new(),
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut loaded = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty() && !value.is_empty() {
                unsafe {
                    std::env::set_var(key, value);
                }
                let masked = if value.len() > 8 {
                    format!("{}...{}", &value[..4], &value[value.len() - 4..])
                } else {
                    "****".to_string()
                };
                loaded.push((key.to_string(), masked));
            }
        }
    }
    loaded
}

/// Save an API key to ~/.abbot/keys.env
pub fn save_api_key(key_name: &str, key_value: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = keys_path().ok_or("could not determine keys path")?;

    let mut lines: Vec<String> = if path.exists() {
        std::fs::read_to_string(&path)?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        vec![
            "# Abbot API Keys".to_string(),
            "# This file is loaded by the daemon on startup".to_string(),
            "".to_string(),
        ]
    };

    let key_line = format!("{}={}", key_name, key_value);
    let mut found = false;
    for line in &mut lines {
        if line.starts_with(&format!("{}=", key_name)) {
            *line = key_line.clone();
            found = true;
            break;
        }
    }
    if !found {
        lines.push(key_line);
    }

    let content = lines.join("\n") + "\n";

    atomic_write_file_0600(&path, &content)?;

    Ok(())
}

/// Remove an API key from ~/.abbot/keys.env
pub fn remove_api_key(key_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = keys_path().ok_or("could not determine keys path")?;

    if !path.exists() {
        return Ok(());
    }

    let lines: Vec<String> = std::fs::read_to_string(&path)?
        .lines()
        .filter(|line| !line.starts_with(&format!("{}=", key_name)))
        .map(|s| s.to_string())
        .collect();

    let content = lines.join("\n") + "\n";
    atomic_write_file_0600(&path, &content)?;
    Ok(())
}

// =============================================================================
// CONFIG MODEL UPDATE
// =============================================================================

/// Update the model in ~/.abbot/abbot.toml for head, hand, and mind.
pub fn update_config_model(
    cli_config: Option<&Path>,
    model: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = resolve_config_path(cli_config).ok_or("could not determine config path")?;

    if !config_path.exists() {
        return Err("config file not found, run 'abbot init' first".into());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut new_lines = Vec::new();

    let model_sections = ["[llm]", "[prompt_cache]"];
    let mut in_model_section = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') {
            in_model_section = model_sections.iter().any(|s| trimmed.starts_with(s));
        }

        if in_model_section && trimmed.starts_with("model = ") {
            let indent = line.len() - line.trim_start().len();
            let whitespace = &line[..indent];
            new_lines.push(format!("{}model = \"{}\"", whitespace, model));
        } else {
            new_lines.push(line.to_string());
        }
    }

    atomic_write_file_0600(&config_path, &(new_lines.join("\n") + "\n"))?;
    Ok(())
}

// =============================================================================
// PROVIDER CACHE
// =============================================================================

#[derive(Serialize, Deserialize, Debug)]
pub struct ProviderCache {
    pub provider: String,
    pub fetched_at: String,
    pub models: Vec<CachedModel>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CachedModel {
    pub id: String,
    pub name: Option<String>,
    pub context_window: Option<u64>,
    #[serde(default)]
    pub input_cost: Option<f64>,
    #[serde(default)]
    pub output_cost: Option<f64>,
}

pub fn load_provider_cache(provider: &str) -> Option<ProviderCache> {
    let path = providers_dir()?.join(format!("{}.json", provider));
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_provider_cache(cache: &ProviderCache) -> Result<(), Box<dyn std::error::Error>> {
    let dir = providers_dir().ok_or("could not determine providers directory")?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", cache.provider));
    let content = serde_json::to_string_pretty(cache)?;
    std::fs::write(&path, content)?;
    Ok(())
}

// =============================================================================
// DEFAULT CONFIG GENERATION
// =============================================================================

/// Generate the default abbot.toml content with all config properties.
///
/// Properties with sensible defaults are set; optional properties without
/// defaults are commented out so users can see what's available.
/// Trait selections as `(category, variant)` pairs.
/// Pass an empty slice for all-none defaults.
pub fn generate_default_config(
    model: &str,
    traits: &[(&str, &str)],
    developer: bool,
    tick_interval: u64,
) -> String {
    use abbot::runtime::trait_catalog::trait_categories;

    // Build the [traits] section lines
    let mut traits_lines = String::new();
    for &(category, variants) in trait_categories() {
        let selected = traits
            .iter()
            .find(|(c, _)| *c == category)
            .map(|(_, v)| *v)
            .unwrap_or("none");

        // Validate: if selected isn't in the variant list and isn't "none", fall back
        let value = if selected == "none" || variants.contains(&selected) {
            selected
        } else {
            "none"
        };

        traits_lines.push_str(&format!("{} = \"{}\"\n", category, value));
    }

    format!(
        r#"# Abbot configuration
developer = {developer}

[server]
addr = "127.0.0.1:8080"
log_format = "default"
# proxy_base_url = ""
# web_dist = ""
allow_loopback_main_scope = false
allow_cors_any = false

[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"

[providers.anthropic]
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"

[providers.openai]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[providers.ollama]
base_url = "http://localhost:11434/v1"
api_key_env = ""

[llm]
model = "{model}"
temperature = 0.7
# max_tokens = 4096

[traits]
{traits}
[head]
heartbeat_tick = 30
debounce_ms = 500
# time_gap_marker_minutes = 5
# pool = 3

[hand]
max_iters = 24
# max_output_chars_in_prompt = 8000
# max_trace_entries_in_prompt = 20
# pool = 4

[mind]
tick_interval = {tick_interval}

[prompt_cache]
# enabled = false
# model = ""
# temperature = 0.7
# max_tokens = 4096

[harness]
# model = ""
# slow_idle = 5
# deep_idle = 60

# [vfs]
# mounts = []
"#,
        developer = developer,
        model = model,
        traits = traits_lines,
        tick_interval = tick_interval,
    )
}

// =============================================================================
// APPCONFIG INITIALIZATION HELPER
// =============================================================================

/// Initialize AppConfig from CLI --config flag or default path.
pub fn init_app_config(cli_config: Option<&std::path::Path>) {
    use abbot::runtime::AppConfig;
    use abbot::runtime::app_config::default_config_path as daemon_config_path;

    if let Some(path) = cli_config {
        AppConfig::init(path);
    } else if let Some(path) = daemon_config_path() {
        if path.exists() {
            AppConfig::init(&path);
        } else {
            AppConfig::init_default();
        }
    } else {
        AppConfig::init_default();
    }
}
