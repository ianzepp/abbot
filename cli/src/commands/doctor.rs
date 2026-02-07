//! Doctor command - Run offline preflight health checks
//!
//! Calls the daemon's `run_preflight()` directly (via crate dependency) to
//! validate config, endpoints, and databases. Prints the resulting
//! `preflight.log` to stdout.

use std::path::PathBuf;

use crate::config;
use crate::error::CliError;

pub async fn run(cli_config: Option<PathBuf>) -> Result<(), CliError> {
    use abbot::runtime::app_config::{self, WorkspacePaths};
    use abbot::runtime::preflight::run_preflight;

    config::load_api_keys();
    config::init_app_config(cli_config.as_deref());

    let home = dirs::home_dir()
        .ok_or_else(|| CliError::General("could not determine home directory".into()))?;
    let paths = WorkspacePaths::new(home);

    // Run preflight checks (writes results to preflight.log)
    match run_preflight(&paths).await {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Preflight error: {e}");
        }
    }

    // Read and display the preflight log
    let log_path = app_config::config_dir()
        .map(|d| d.join("preflight.log"))
        .filter(|p| p.exists());

    match log_path {
        Some(path) => {
            let content = std::fs::read_to_string(&path).map_err(|e| {
                CliError::General(format!("failed to read {}: {}", path.display(), e))
            })?;
            print!("{}", content);
        }
        None => {
            println!("No preflight log generated.");
        }
    }

    Ok(())
}
