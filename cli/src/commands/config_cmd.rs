//! Config command - Read and write ~/.abbot/abbot.toml

use clap::Subcommand;

use crate::config;
use crate::error::CliError;
use crate::output::{OutputFormat, print_value};

#[derive(Debug, Subcommand, Clone)]
pub enum ConfigAction {
    /// Show configuration (all sections or a specific section)
    Get {
        /// Section name: head, hand, mind, server, pool, harness, etc.
        section: Option<String>,
    },
    /// Set configuration values within a section
    Set {
        /// Section name: head, hand, mind, server, pool, harness, etc.
        section: String,
        /// Key=value pairs (e.g. model=openai/gpt-5.1 temperature=0.7)
        #[arg(required = true)]
        values: Vec<String>,
    },
}

pub fn run(
    cli_config: Option<std::path::PathBuf>,
    action: ConfigAction,
    format: OutputFormat,
) -> Result<(), CliError> {
    let config_path = config::resolve_config_path(cli_config.as_deref())
        .ok_or_else(|| CliError::Config("could not determine config path".into()))?;

    match action {
        ConfigAction::Get { section } => {
            if !config_path.exists() {
                return Err(CliError::Config(format!(
                    "config file not found: {}",
                    config_path.display()
                )));
            }

            let content = std::fs::read_to_string(&config_path)?;
            let table: toml::Value = content
                .parse()
                .map_err(|e: toml::de::Error| CliError::Config(format!("parse error: {e}")))?;

            let value = match section {
                None => toml_to_json(&table),
                Some(ref name) => {
                    let toml::Value::Table(ref map) = table else {
                        return Err(CliError::Config("config is not a table".into()));
                    };
                    match map.get(name.as_str()) {
                        Some(v) => toml_to_json(v),
                        None => {
                            return Err(CliError::Config(format!("unknown section: {name}")));
                        }
                    }
                }
            };

            print_value(&value, format);
        }

        ConfigAction::Set { section, values } => {
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

            let section_table = table
                .entry(&section)
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

            let toml::Value::Table(sec) = section_table else {
                return Err(CliError::Config(format!(
                    "section '{section}' is not a table"
                )));
            };

            for kv in &values {
                let (key, raw_val) = kv
                    .split_once('=')
                    .ok_or_else(|| CliError::General(format!("invalid key=value pair: {kv}")))?;

                let key = key.trim();
                let raw_val = raw_val.trim();

                if raw_val.is_empty() {
                    sec.remove(key);
                } else {
                    sec.insert(key.to_string(), parse_toml_value(raw_val));
                }
            }

            let output = toml::to_string_pretty(&toml::Value::Table(table))
                .map_err(|e| CliError::General(format!("serialize error: {e}")))?;

            abbot::runtime::app_config::atomic_write_file_0600(&config_path, &output)?;

            print_value(
                &serde_json::json!({
                    "path": config_path.display().to_string(),
                    "section": section,
                    "updated": values,
                }),
                format,
            );
        }
    }

    Ok(())
}

/// Parse a raw string value into the most specific TOML type.
fn parse_toml_value(raw: &str) -> toml::Value {
    // Boolean
    if raw == "true" {
        return toml::Value::Boolean(true);
    }
    if raw == "false" {
        return toml::Value::Boolean(false);
    }

    // Integer
    if let Ok(i) = raw.parse::<i64>() {
        return toml::Value::Integer(i);
    }

    // Float
    if let Ok(f) = raw.parse::<f64>() {
        return toml::Value::Float(f);
    }

    // String (strip surrounding quotes if present)
    let s = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(raw);
    toml::Value::String(s.to_string())
}

/// Convert a toml::Value to serde_json::Value for output.
fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::json!(i),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml::Value::Array(arr) => serde_json::Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(map) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
    }
}
