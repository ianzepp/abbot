use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

#[derive(Debug, Deserialize)]
struct ConfigUpdateArgs {
    section: String,
    key: String,
    value: serde_json::Value,
}

pub struct ConfigUpdate;

impl ConfigUpdate {
    pub fn new() -> Self {
        Self
    }
}

fn allowed(section: &str, key: &str) -> bool {
    match section {
        "harness" => matches!(key, "slow_idle" | "deep_idle"),
        "head" => matches!(
            key,
            "temperature"
                | "max_tokens"
                | "fever"
                | "generation"
                | "autist"
                | "heartbeat_tick"
                | "debounce_ms"
                | "time_gap_marker_minutes"
        ),
        "hand" => matches!(
            key,
            "temperature"
                | "max_tokens"
                | "fever"
                | "generation"
                | "autist"
                | "max_iters"
                | "max_output_chars_in_prompt"
                | "max_trace_entries_in_prompt"
        ),
        "mind" => matches!(
            key,
            "temperature"
                | "max_tokens"
                | "fever"
                | "generation"
                | "autist"
                | "tact"
                | "tick_interval"
        ),
        "tars" => matches!(
            key,
            "humor"
                | "honesty"
                | "sarcasm"
                | "verbosity"
                | "confidence"
                | "curiosity"
                | "patience"
                | "formality"
                | "empathy"
                | "pedantry"
                | "initiative"
                | "optimism"
                | "caution"
        ),
        _ => false,
    }
}

fn json_to_toml(v: &serde_json::Value) -> toml::Value {
    match v {
        serde_json::Value::Null => toml::Value::String(String::new()),
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Array(arr) => {
            toml::Value::Array(arr.iter().map(json_to_toml).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut table = toml::Table::new();
            for (k, val) in obj {
                table.insert(k.clone(), json_to_toml(val));
            }
            toml::Value::Table(table)
        }
    }
}

#[async_trait]
impl Syscall for ConfigUpdate {
    fn name(&self) -> &'static str {
        "config:update"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;

        let args: ConfigUpdateArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let section = args.section.trim();
        let key = args.key.trim();
        if section.is_empty() || key.is_empty() {
            return Err(KernelError::invalid_args("section/key is empty"));
        }

        if !allowed(section, key) {
            return Err(KernelError::invalid_args(format!(
                "config key not writable: {}.{}",
                section, key
            )));
        }

        // Only scalar values are allowed.
        match &args.value {
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {}
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                return Err(KernelError::invalid_args(
                    "value must be string/number/bool/null",
                ));
            }
        }

        let config_path = crate::runtime::workspace_config_from_root(&ctx.cwd);

        let config_str = match crate::runtime::read_optional_file(&config_path) {
            Ok(Some(s)) => s,
            Ok(None) => String::new(),
            Err(e) => return Err(KernelError::io(format!("failed to read config: {e}"))),
        };

        let mut config: toml::Table = if config_str.is_empty() {
            toml::Table::new()
        } else {
            config_str
                .parse()
                .map_err(|e| KernelError::io(format!("invalid config TOML: {e}")))?
        };

        let section_table = config
            .entry(section)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut();

        let Some(section_table) = section_table else {
            return Err(KernelError::invalid_args(format!(
                "section '{}' is not a table",
                args.section
            )));
        };

        if args.value.is_null() {
            section_table.remove(key);
        } else {
            let toml_value = json_to_toml(&args.value);
            section_table.insert(key.to_string(), toml_value);
        }

        let new_config_str = toml::to_string_pretty(&config).unwrap_or_default();
        if let Err(e) = crate::runtime::atomic_write_file_0600(&config_path, &new_config_str) {
            return Err(KernelError::io(format!("failed to write config: {e}")));
        }

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "section": section,
                    "key": key,
                    "value": args.value,
                    "status": "updated"
                }),
            ))
            .await;

        Ok(())
    }
}
