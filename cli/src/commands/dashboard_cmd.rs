//! Dashboard command - Launch the abbot-monitor process

use std::path::PathBuf;

use crate::config;
use crate::error::CliError;

pub fn run(
    cli_config: Option<PathBuf>,
    addr: Option<String>,
    args: Vec<String>,
) -> Result<(), CliError> {
    use abbot::runtime::AppConfig;

    config::init_app_config(cli_config.as_deref());

    let bind_addr = addr
        .or_else(|| AppConfig::global().server.addr.clone())
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    // Look for abbot-monitor in the same directory as the current executable first
    let monitor_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("abbot-monitor")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("abbot-monitor"));

    let mut cmd = std::process::Command::new(&monitor_path);
    cmd.arg("--addr").arg(&bind_addr);
    cmd.args(&args);

    let status = cmd.status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}
