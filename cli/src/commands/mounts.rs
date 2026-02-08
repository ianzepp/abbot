//! Mounts command - Manage VFS mount points in ~/.abbot/abbot.toml

use clap::Subcommand;
use serde_json::json;

use crate::config;
use crate::error::CliError;
use crate::output::{OutputFormat, print_value};

#[derive(Debug, Subcommand, Clone)]
pub enum MountsAction {
    /// List configured VFS mount points
    List,
    /// Add a VFS mount point
    Add {
        /// VFS prefix path (e.g., /projects, /data)
        prefix: String,
        /// Host filesystem path (e.g., ~/github/ianzepp, /data/shared)
        host: String,
        /// Mount as read-only
        #[arg(long)]
        ro: bool,
    },
    /// Remove a VFS mount point by prefix
    Remove {
        /// VFS prefix to remove (e.g., /projects)
        prefix: String,
    },
}

pub fn run(
    cli_config: Option<std::path::PathBuf>,
    action: MountsAction,
    format: OutputFormat,
) -> Result<(), CliError> {
    let config_path = config::resolve_config_path(cli_config.as_deref())
        .ok_or_else(|| CliError::Config("could not determine config path".into()))?;

    match action {
        MountsAction::List => {
            if !config_path.exists() {
                return Err(CliError::Config(format!(
                    "config file not found: {}",
                    config_path.display()
                )));
            }

            let content = std::fs::read_to_string(&config_path)?;
            let doc: toml::Value = content
                .parse()
                .map_err(|e: toml::de::Error| CliError::Config(format!("parse error: {e}")))?;

            let mounts: Vec<serde_json::Value> = doc
                .get("vfs")
                .and_then(|v| v.get("mounts"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|m| {
                            json!({
                                "prefix": m.get("prefix").and_then(|v| v.as_str()).unwrap_or(""),
                                "host": m.get("host").and_then(|v| v.as_str()).unwrap_or(""),
                                "mode": m.get("mode").and_then(|v| v.as_str()).unwrap_or("rw"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            print_value(
                &json!({
                    "mounts": mounts,
                    "count": mounts.len(),
                }),
                format,
            );
        }

        MountsAction::Add { prefix, host, ro } => {
            // Normalize prefix: ensure it starts with /
            let prefix = if prefix.starts_with('/') {
                prefix
            } else {
                format!("/{prefix}")
            };

            // Validate: prefix "/" is forbidden (VFS root is always memory-backed)
            if prefix == "/" {
                return Err(CliError::General(
                    "cannot mount at '/': VFS root is always memory-backed".into(),
                ));
            }

            // Validate: host path must start with ~ or /
            if !host.starts_with('~') && !host.starts_with('/') {
                return Err(CliError::General(
                    "host path must be absolute (start with / or ~)".into(),
                ));
            }

            // Expand and validate host path exists
            let expanded = abbot::vfs::expand_host_path(&host)
                .map_err(|e| CliError::General(format!("invalid host path: {e}")))?;

            if !expanded.exists() {
                return Err(CliError::General(format!(
                    "host path does not exist: {}",
                    expanded.display()
                )));
            }

            if !expanded.is_dir() {
                return Err(CliError::General(format!(
                    "host path is not a directory: {}",
                    expanded.display()
                )));
            }

            let mode = if ro { "ro" } else { "rw" };

            // Read and update config
            let content = if config_path.exists() {
                std::fs::read_to_string(&config_path)?
            } else {
                String::new()
            };

            let mut table: toml::map::Map<String, toml::Value> = content
                .parse::<toml::Value>()
                .map_err(|e: toml::de::Error| CliError::Config(format!("parse error: {e}")))?
                .as_table()
                .cloned()
                .unwrap_or_default();

            // Get or create [vfs] section
            let vfs = table
                .entry("vfs")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

            let toml::Value::Table(vfs_table) = vfs else {
                return Err(CliError::Config("'vfs' is not a table".into()));
            };

            // Get or create mounts array
            let mounts = vfs_table
                .entry("mounts")
                .or_insert_with(|| toml::Value::Array(Vec::new()));

            let toml::Value::Array(mounts_arr) = mounts else {
                return Err(CliError::Config("'vfs.mounts' is not an array".into()));
            };

            // Check for duplicate prefix
            let exists = mounts_arr.iter().any(|m| {
                m.get("prefix")
                    .and_then(|v| v.as_str())
                    .is_some_and(|p| p == prefix)
            });

            if exists {
                return Err(CliError::General(format!(
                    "mount prefix already exists: {prefix}"
                )));
            }

            // Build new mount entry
            let mut entry = toml::map::Map::new();
            entry.insert("prefix".into(), toml::Value::String(prefix.clone()));
            entry.insert("host".into(), toml::Value::String(host.clone()));
            if ro {
                entry.insert("mode".into(), toml::Value::String(mode.into()));
            }

            mounts_arr.push(toml::Value::Table(entry));

            // Write back
            let output = toml::to_string_pretty(&toml::Value::Table(table))
                .map_err(|e| CliError::General(format!("serialize error: {e}")))?;

            abbot::runtime::app_config::atomic_write_file_0600(&config_path, &output)?;

            print_value(
                &json!({
                    "status": "added",
                    "prefix": prefix,
                    "host": host,
                    "mode": mode,
                    "message": "Restart abbot for changes to take effect.",
                }),
                format,
            );
        }

        MountsAction::Remove { prefix } => {
            if !config_path.exists() {
                return Err(CliError::Config(format!(
                    "config file not found: {}",
                    config_path.display()
                )));
            }

            // Normalize prefix
            let prefix = if prefix.starts_with('/') {
                prefix
            } else {
                format!("/{prefix}")
            };

            let content = std::fs::read_to_string(&config_path)?;
            let mut table: toml::map::Map<String, toml::Value> = content
                .parse::<toml::Value>()
                .map_err(|e: toml::de::Error| CliError::Config(format!("parse error: {e}")))?
                .as_table()
                .cloned()
                .unwrap_or_default();

            let removed = if let Some(toml::Value::Table(vfs)) = table.get_mut("vfs") {
                if let Some(toml::Value::Array(mounts)) = vfs.get_mut("mounts") {
                    let before = mounts.len();
                    mounts.retain(|m| {
                        m.get("prefix")
                            .and_then(|v| v.as_str())
                            .is_none_or(|p| p != prefix)
                    });
                    before != mounts.len()
                } else {
                    false
                }
            } else {
                false
            };

            if !removed {
                return Err(CliError::General(format!(
                    "no mount found with prefix: {prefix}"
                )));
            }

            let output = toml::to_string_pretty(&toml::Value::Table(table))
                .map_err(|e| CliError::General(format!("serialize error: {e}")))?;

            abbot::runtime::app_config::atomic_write_file_0600(&config_path, &output)?;

            print_value(
                &json!({
                    "status": "removed",
                    "prefix": prefix,
                    "message": "Restart abbot for changes to take effect.",
                }),
                format,
            );
        }
    }

    Ok(())
}
