use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

#[derive(Debug, Deserialize)]
struct ConfigReadArgs {
    #[serde(default)]
    section: Option<String>,
    #[serde(default)]
    key: Option<String>,
}

pub struct ConfigRead;

impl ConfigRead {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ConfigRead {
    fn name(&self) -> &'static str {
        "config:read"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: ConfigReadArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let config_path = crate::runtime::workspace_config_from_root(&ctx.cwd);

        let config_str = match crate::runtime::read_optional_file(&config_path) {
            Ok(Some(s)) => s,
            Ok(None) => String::new(),
            Err(e) => return Err(KernelError::io(format!("failed to read config: {e}"))),
        };

        let config: toml::Table = if config_str.is_empty() {
            toml::Table::new()
        } else {
            config_str
                .parse()
                .map_err(|e| KernelError::io(format!("invalid config TOML: {e}")))?
        };

        let result = match (args.section.as_deref(), args.key.as_deref()) {
            (None, None) => json!(config),
            (Some(section), None) => {
                let value = config
                    .get(section)
                    .cloned()
                    .unwrap_or(toml::Value::Table(toml::Table::new()));
                json!({ "section": section, "value": value })
            }
            (Some(section), Some(key)) => {
                let value = config
                    .get(section)
                    .and_then(|s| s.as_table())
                    .and_then(|t| t.get(key))
                    .cloned();
                json!({ "section": section, "key": key, "value": value })
            }
            (None, Some(_)) => {
                return Err(KernelError::invalid_args("key requires section"));
            }
        };

        let _ = tx.send(Frame::ok(ctx.call_id, result)).await;
        Ok(())
    }
}
