//! Plugin command - Manage workspace plugins (tools)

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Subcommand;

use crate::config;
use crate::error::CliError;

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
    std::fs::write(config_path, out)?;
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

pub fn run(cli_config: Option<PathBuf>, action: PluginAction) -> Result<(), CliError> {
    use abbot::runtime::AppConfig;
    use abbot::runtime::PluginManager;

    config::init_app_config(cli_config.as_deref());

    let config_path =
        config::default_config_path().ok_or(CliError::General("could not determine config path".into()))?;
    let plugins_config = read_plugins_config(&config_path);

    let workspace = AppConfig::global().workspace_path().ok();
    let workspace_root = workspace.as_ref().map(|w| w.join("root"));

    match action {
        PluginAction::Detect => {
            println!("Detecting installed plugins...\n");

            let mgr = workspace_root
                .as_ref()
                .map(|r| PluginManager::load_for_workspace_root(r))
                .unwrap_or_else(PluginManager::empty);

            let catalog = mgr.catalog();
            if catalog.is_empty() {
                println!("No built-in plugins found.");
                return Ok(());
            }

            let mut results: Vec<(String, String, bool, Option<String>)> = Vec::new();

            for p in &catalog {
                let (installed, version) = detect_program(&p.program);
                results.push((p.id.clone(), p.program.clone(), installed, version));
            }

            for (id, _program, installed, version) in &results {
                let status = if *installed { "found" } else { "not found" };
                let ver = version.as_deref().unwrap_or("");
                let current = plugins_config.get(id).map(|l| l.as_str()).unwrap_or("-");
                println!("  {:<12} {:<10} {:<10} {}", id, status, current, ver);
            }

            let found_count = results.iter().filter(|(_, _, i, _)| *i).count();
            println!("\n{}/{} plugins available", found_count, results.len());
            println!("\nSet access level: abbot plugin set <name> <none|read|write>");
        }

        PluginAction::Set { name, level } => {
            let level = PluginLevel::from_str(&level).ok_or_else(|| {
                CliError::General(format!(
                    "invalid level '{}', use: none, read, write",
                    level
                ))
            })?;

            update_plugin_level(&config_path, &name, &level)?;
            println!("Set {} = {}", name, level.as_str());
            println!("Restart abbot to apply changes.");
        }

        PluginAction::List => {
            let mgr = workspace_root
                .as_ref()
                .map(|r| PluginManager::load_for_workspace_root(r))
                .unwrap_or_else(PluginManager::empty);

            let catalog = mgr.catalog();

            println!("Plugins:\n");
            if catalog.is_empty() {
                println!("(no built-in plugins available)");
                return Ok(());
            }

            for p in &catalog {
                let level = plugins_config.get(&p.id).map(|l| l.as_str()).unwrap_or("-");
                println!("  {:<12} {:<10} {}", p.id, level, p.description);
            }

            println!("\nLevels: none (disabled), read (safe), write (full)");
            println!("Set: abbot plugin set <name> <none|read|write>");
            println!("Detect: abbot plugin detect");
        }
    }

    Ok(())
}
