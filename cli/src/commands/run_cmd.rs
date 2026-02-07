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
    let mut cmd = std::process::Command::new("opencode");
    cmd.env("LOCAL_ENDPOINT", format!("{}/v1", base_url));
    cmd.args(args);

    let status = cmd.status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}
