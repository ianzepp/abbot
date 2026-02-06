//! Plugin command - Manage workspace plugins (tools)

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;

use crate::config;
use crate::error::CliError;
use crate::output::{print_value, OutputFormat};

use abbot::runtime::app_config::atomic_write_file_0600;

#[derive(Debug, Subcommand, Clone)]
pub enum PluginAction {
    /// Detect installed plugins and show versions
    Detect,
    /// Set plugin access level (none, read, write)
    Set {
        /// Plugin name
        name: String,
        /// Access level: none, read, write
        level: String,
    },
    /// List available plugins and their status
    List,
}

#[derive(Debug, Clone, PartialEq)]
enum PluginLevel {
    None,
    Read,
    Write,
}

impl PluginLevel {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "none" => Some(Self::None),
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

fn read_plugins_config(config_path: &std::path::Path) -> HashMap<String, PluginLevel> {
    let raw = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };

    let v: toml::Value = match toml::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };

    let mut out = HashMap::new();
    if let Some(plugins) = v.get("plugins").and_then(|v| v.as_table()) {
        for (id, value) in plugins {
            if let Some(level_str) = value.as_str() {
                if let Some(level) = PluginLevel::from_str(level_str) {
                    out.insert(id.clone(), level);
                }
            }
        }
    }
    out
}

fn update_plugin_level(
    config_path: &std::path::Path,
    plugin_id: &str,
    level: &PluginLevel,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc: toml::Table = toml::from_str(&raw).unwrap_or_default();

    let plugins = doc
        .entry("plugins".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or("plugins is not a table")?;

    plugins.insert(
        plugin_id.to_string(),
        toml::Value::String(level.as_str().to_string()),
    );

    let out = toml::to_string_pretty(&doc)?;
    atomic_write_file_0600(config_path, &out)?;
    Ok(())
}

fn detect_program(program: &str) -> (bool, Option<String>) {
    let which = std::process::Command::new("which").arg(program).output();

    let installed = which.map(|o| o.status.success()).unwrap_or(false);
    if !installed {
        return (false, None);
    }

    for flag in ["--version", "-V", "version"] {
        if let Ok(output) = std::process::Command::new(program).arg(flag).output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let text = if stdout.trim().is_empty() {
                    stderr
                } else {
                    stdout
                };
                let version = text.lines().next().unwrap_or("").trim().to_string();
                if !version.is_empty() {
                    return (true, Some(version));
                }
            }
        }
    }

    (true, None)
}

pub fn run(
    cli_config: Option<PathBuf>,
    action: PluginAction,
    format: OutputFormat,
) -> Result<(), CliError> {
    use abbot::runtime::AppConfig;
    use abbot::runtime::PluginManager;

    config::init_app_config(cli_config.as_deref());

    let config_path = config::resolve_config_path(cli_config.as_deref())
        .ok_or(CliError::General("could not determine config path".into()))?;
    let plugins_config = read_plugins_config(&config_path);

    let workspace = AppConfig::global().workspace_path().ok();
    let workspace_root = workspace.as_ref().map(|w| w.join("root"));

    match action {
        PluginAction::Detect => {
            let mgr = workspace_root
                .as_ref()
                .map(|r| PluginManager::load_for_workspace_root(r))
                .unwrap_or_else(PluginManager::empty);

            let catalog = mgr.catalog();
            if catalog.is_empty() {
                print_value(
                    &json!({ "plugins": [], "available": 0, "total": 0 }),
                    format,
                );
                return Ok(());
            }

            let mut results: Vec<serde_json::Value> = Vec::new();

            for p in &catalog {
                let (installed, version) = detect_program(&p.program);
                let current = plugins_config.get(&p.id).map(|l| l.as_str()).unwrap_or("-");
                results.push(json!({
                    "id": p.id,
                    "program": p.program,
                    "installed": installed,
                    "version": version,
                    "level": current,
                }));
            }

            let found_count = results
                .iter()
                .filter(|r| r["installed"].as_bool() == Some(true))
                .count();

            print_value(
                &json!({
                    "plugins": results,
                    "available": found_count,
                    "total": catalog.len(),
                }),
                format,
            );
        }

        PluginAction::Set { name, level } => {
            let level = PluginLevel::from_str(&level).ok_or_else(|| {
                CliError::General(format!("invalid level '{}', use: none, read, write", level))
            })?;

            update_plugin_level(&config_path, &name, &level)?;

            print_value(
                &json!({
                    "plugin": name,
                    "level": level.as_str(),
                    "message": "Restart abbot to apply changes.",
                }),
                format,
            );
        }

        PluginAction::List => {
            let mgr = workspace_root
                .as_ref()
                .map(|r| PluginManager::load_for_workspace_root(r))
                .unwrap_or_else(PluginManager::empty);

            let catalog = mgr.catalog();

            let plugins: Vec<serde_json::Value> = catalog
                .iter()
                .map(|p| {
                    let level = plugins_config.get(&p.id).map(|l| l.as_str()).unwrap_or("-");
                    json!({
                        "id": p.id,
                        "level": level,
                        "description": p.description,
                    })
                })
                .collect();

            print_value(&json!({ "plugins": plugins }), format);
        }
    }

    Ok(())
}
