//! Run command - Launch external tools with Abbot as the API provider

use std::path::PathBuf;

use clap::Subcommand;

use crate::config;
use crate::error::CliError;

#[derive(Subcommand)]
pub enum RunTarget {
    /// Launch Claude Code with Abbot as the API endpoint
    Claude {
        /// Additional arguments to pass to claude
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Launch OpenCode with Abbot as the API endpoint
    Opencode {
        /// Additional arguments to pass to opencode
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

pub fn run(cli_config: Option<PathBuf>, target: RunTarget) -> Result<(), CliError> {
    use abbot::runtime::AppConfig;

    config::init_app_config(cli_config.as_deref());

    let bind_addr = AppConfig::global()
        .server
        .addr
        .clone()
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());
    let base_url = format!("http://{}", bind_addr);

    match target {
        RunTarget::Claude { args } => run_claude(&base_url, &args),
        RunTarget::Opencode { args } => run_opencode(&base_url, &args),
    }
}

fn run_claude(base_url: &str, args: &[String]) -> Result<(), CliError> {
    let mut cmd = std::process::Command::new("claude");
    cmd.env("ANTHROPIC_BASE_URL", base_url);
    cmd.args(args);

    let status = cmd.status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn run_opencode(base_url: &str, args: &[String]) -> Result<(), CliError> {
    // Write/merge the Abbot provider block into OpenCode's config
    if let Err(e) = update_opencode_config(base_url) {
        eprintln!("Warning: failed to update opencode config: {}", e);
    }

    let mut cmd = std::process::Command::new("opencode");
    cmd.arg("-m").arg("abbot/abbot/default");
    cmd.args(args);

    let status = cmd.status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn update_opencode_config(base_url: &str) -> Result<(), CliError> {
    let api_url = format!("{}/v1", base_url);

    let config_dir = dirs::home_dir()
        .ok_or_else(|| CliError::General("could not determine home directory".into()))?
        .join(".config")
        .join("opencode");

    std::fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("opencode.json");

    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if config.get("provider").is_none() {
        config["provider"] = serde_json::json!({});
    }

    config["provider"]["abbot"] = serde_json::json!({
        "name": "Abbot",
        "npm": "@ai-sdk/openai-compatible",
        "options": {
            "baseURL": api_url,
            "apiKey": "not-required"
        },
        "models": {
            "abbot/default": {
                "name": "Abbot Default"
            }
        }
    });

    let content =
        serde_json::to_string_pretty(&config).map_err(|e| CliError::General(e.to_string()))?;
    std::fs::write(&config_path, content + "\n")?;

    Ok(())
}
