//! Use command - Switch the active LLM provider/model and run preflight
//!
//! Updates the `[llm]` model in `~/.abbot/abbot.toml` and runs offline
//! preflight checks to verify the new configuration works.

use std::path::PathBuf;

use crate::config;
use crate::error::CliError;

pub async fn run(
    cli_config: Option<PathBuf>,
    provider: String,
    model: String,
) -> Result<(), CliError> {
    let model_id = format!("{}/{}", provider.trim(), model.trim());

    // Update config file
    config::update_config_model(cli_config.as_deref(), &model_id)
        .map_err(|e| CliError::General(format!("failed to update config: {e}")))?;

    println!("model = \"{}\"", model_id);

    // Re-init AppConfig with the updated file, then run preflight
    config::load_api_keys();
    config::init_app_config(cli_config.as_deref());

    use abbot::runtime::app_config::WorkspacePaths;
    use abbot::runtime::preflight::run_preflight;

    let home = dirs::home_dir()
        .ok_or_else(|| CliError::General("could not determine home directory".into()))?;
    let paths = WorkspacePaths::new(home);

    match run_preflight(&paths).await {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Preflight error: {e}");
        }
    }

    // Read and display the preflight log
    let log_path = abbot::runtime::app_config::config_dir()
        .map(|d| d.join("preflight.log"))
        .filter(|p| p.exists());

    match log_path {
        Some(path) => {
            let content = std::fs::read_to_string(&path).map_err(|e| {
                CliError::General(format!("failed to read {}: {}", path.display(), e))
            })?;
            print!("{}", crate::output::colorize_preflight(&content));
        }
        None => {
            println!("No preflight log generated.");
        }
    }

    Ok(())
}
