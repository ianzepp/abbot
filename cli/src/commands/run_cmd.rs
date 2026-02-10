//! Run command - Launch external tools with Abbot as the API provider

use std::path::PathBuf;

use clap::Subcommand;

use crate::config;
use crate::error::CliError;

#[derive(Subcommand)]
pub enum RunTarget {
    /// Launch the TUI (assumes daemon is already running)
    Tui {
        /// Additional arguments to pass to abbot-tui
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Launch the monitoring dashboard (assumes daemon is already running)
    Monitor {
        /// Additional arguments to pass to abbot-monitor
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
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

    let developer = AppConfig::global().developer.unwrap_or(false);

    match target {
        RunTarget::Tui { args } => {
            let mut full_args = args;
            if developer {
                full_args.insert(0, "--developer".to_string());
            }
            super::tui_cmd::run(cli_config, Some(bind_addr), full_args)
        }
        RunTarget::Monitor { args } => {
            if !developer {
                return Err(CliError::General(
                    "'abbot run monitor' requires developer mode (developer = true in abbot.toml)"
                        .into(),
                ));
            }
            run_monitor(&bind_addr, &args)
        }
        RunTarget::Claude { args } => run_claude(&base_url, &args),
        RunTarget::Opencode { args } => run_opencode(&base_url, &args),
    }
}

fn run_monitor(bind_addr: &str, args: &[String]) -> Result<(), CliError> {
    // Look for abbot-monitor next to the current executable first, then fall back to PATH
    let monitor_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("abbot-monitor")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("abbot-monitor"));

    let mut cmd = std::process::Command::new(&monitor_path);
    cmd.arg("--addr").arg(bind_addr);
    cmd.args(args);

    let status = cmd.status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
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
