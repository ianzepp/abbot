//! Config command - Read and write ~/.abbot/abbot.toml

use std::path::Path;

use clap::Subcommand;
use inquire::{Select, Text};

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
    action: Option<ConfigAction>,
    format: OutputFormat,
) -> Result<(), CliError> {
    let config_path = config::resolve_config_path(cli_config.as_deref())
        .ok_or_else(|| CliError::Config("could not determine config path".into()))?;

    let Some(action) = action else {
        return run_interactive(&config_path);
    };

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

// =============================================================================
// INTERACTIVE EDITOR
// =============================================================================

const DONE: &str = "Done";
const BACK: &str = "Back";

fn run_interactive(config_path: &Path) -> Result<(), CliError> {
    let content = if config_path.exists() {
        std::fs::read_to_string(config_path)?
    } else {
        String::new()
    };

    let mut table: toml::map::Map<String, toml::Value> = content
        .parse::<toml::Value>()
        .map_err(|e: toml::de::Error| CliError::Config(format!("parse error: {e}")))?
        .as_table()
        .cloned()
        .unwrap_or_default();

    loop {
        // Build section options: "Done", then scalars as "key = value", then table names
        let mut options = vec![DONE.to_string()];

        for (key, value) in &table {
            match value {
                toml::Value::Table(_) => options.push(key.clone()),
                _ => options.push(format!("{key} = {}", format_value(value))),
            }
        }

        let choice = Select::new("Select a section:", options)
            .prompt()
            .map_err(|e| CliError::General(format!("{e}")))?;

        if choice == DONE {
            save(&table, config_path)?;
            return Ok(());
        }

        // Determine if the user selected a scalar or a table section
        let key = choice.split(" = ").next().unwrap_or(&choice).to_string();

        match table.get(&key).cloned() {
            Some(toml::Value::Table(sec)) => {
                edit_section(&key, sec, &mut table, config_path)?;
            }
            Some(_) => {
                edit_scalar(&key, &mut table, config_path)?;
            }
            None => {}
        }
    }
}

fn edit_section(
    section: &str,
    mut sec: toml::map::Map<String, toml::Value>,
    table: &mut toml::map::Map<String, toml::Value>,
    config_path: &Path,
) -> Result<(), CliError> {
    loop {
        let mut options = vec![BACK.to_string()];

        for (key, value) in &sec {
            options.push(format!("{key} = {}", format_value(value)));
        }

        let prompt = format!("Select a property: [{section}]");
        let choice = Select::new(&prompt, options)
            .prompt()
            .map_err(|e| CliError::General(format!("{e}")))?;

        if choice == BACK {
            table.insert(section.to_string(), toml::Value::Table(sec));
            return Ok(());
        }

        let prop_key = choice.split(" = ").next().unwrap().to_string();

        // Skip arrays — they aren't editable inline
        if let Some(toml::Value::Array(_)) = sec.get(&prop_key) {
            println!("  (array values cannot be edited here; use `abbot config set`)");
            continue;
        }

        let current = sec.get(&prop_key).map(format_value).unwrap_or_default();

        let prompt = format!("New value for \"{prop_key}\" (empty to remove):");
        let new_val = Text::new(&prompt)
            .with_default(&current)
            .prompt()
            .map_err(|e| CliError::General(format!("{e}")))?;

        let new_val = new_val.trim();
        if new_val.is_empty() {
            sec.remove(&prop_key);
            println!("  Removed: {section}.{prop_key}");
        } else {
            let parsed = parse_toml_value(new_val);
            println!(
                "  Updated: {section}.{prop_key} = {}",
                format_value(&parsed)
            );
            sec.insert(prop_key, parsed);
        }

        // Save after each edit
        table.insert(section.to_string(), toml::Value::Table(sec.clone()));
        save(table, config_path)?;
    }
}

fn edit_scalar(
    key: &str,
    table: &mut toml::map::Map<String, toml::Value>,
    config_path: &Path,
) -> Result<(), CliError> {
    // Skip arrays
    if let Some(toml::Value::Array(_)) = table.get(key) {
        println!("  (array values cannot be edited here; use `abbot config set`)");
        return Ok(());
    }

    let current = table.get(key).map(format_value).unwrap_or_default();

    let prompt = format!("New value for \"{key}\" (empty to remove):");
    let new_val = Text::new(&prompt)
        .with_default(&current)
        .prompt()
        .map_err(|e| CliError::General(format!("{e}")))?;

    let new_val = new_val.trim();
    if new_val.is_empty() {
        table.remove(key);
        println!("  Removed: {key}");
    } else {
        let parsed = parse_toml_value(new_val);
        println!("  Updated: {key} = {}", format_value(&parsed));
        table.insert(key.to_string(), parsed);
    }

    save(table, config_path)?;
    Ok(())
}

fn format_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("\"{s}\""),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::Array(arr) => format!("[{} items]", arr.len()),
        toml::Value::Table(map) => format!("{{{} keys}}", map.len()),
    }
}

fn save(table: &toml::map::Map<String, toml::Value>, config_path: &Path) -> Result<(), CliError> {
    let output = toml::to_string_pretty(&toml::Value::Table(table.clone()))
        .map_err(|e| CliError::General(format!("serialize error: {e}")))?;
    abbot::runtime::app_config::atomic_write_file_0600(config_path, &output)?;
    Ok(())
}

// =============================================================================
// HELPERS
// =============================================================================

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
