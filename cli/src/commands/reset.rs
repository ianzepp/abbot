//! Reset command - Delete workspace state with confirmation

use std::path::PathBuf;

use crate::config;
use crate::error::CliError;

pub fn run(cli_config: Option<PathBuf>, force: bool, reset_config: bool) -> Result<(), CliError> {
    use abbot::runtime::AppConfig;
    use abbot::runtime::app_config::WorkspacePaths;
    use inquire::Confirm;

    config::init_app_config(cli_config.as_deref());

    let workspace = AppConfig::global().workspace_path().ok();
    let config_path = cli_config.or_else(config::default_config_path);

    // Show what will be deleted
    println!("This will delete:\n");

    if let Some(ref ws) = workspace {
        if ws.exists() {
            let paths = WorkspacePaths::new(ws.clone());
            for (name, path) in [
                ("store.db (conversation history)", &paths.store_db),
                ("recall.db (memory embeddings)", &paths.recall_db),
                ("ems.db (entity storage)", &paths.ems_db),
                ("frames.db (frame history)", &paths.frames_db),
            ] {
                if path.exists() {
                    println!("  {}", name);
                }
            }

            let mind_memory = paths.mind.join("memory.md");
            if mind_memory.exists() {
                println!("  mind/memory.md (long-term memory)");
            }

            let mind_self = paths.mind.join("self.md");
            if mind_self.exists() {
                println!("  mind/self.md (collective identity)");
            }

            let head_dir = ws.join("head");
            if head_dir.exists() {
                println!("  head/ (head state)");
            }

            let plugins_path = ws.join("plugins.toml");
            if plugins_path.exists() {
                println!("  plugins.toml");
            }

            let workspace_config = ws.join("config.toml");
            if workspace_config.exists() {
                println!("  config.toml (workspace config)");
            }
        } else {
            println!("  (workspace does not exist: {})", ws.display());
        }
    } else {
        println!("  (no workspace configured)");
    }

    if reset_config {
        if let Some(ref cp) = config_path {
            if cp.exists() {
                println!("  ~/.config/abbot/abbot.toml (will be regenerated)");
            }
        }
    }

    println!();

    // Confirm unless --force
    if !force {
        let confirm = Confirm::new("Are you sure you want to reset?")
            .with_default(false)
            .prompt()
            .map_err(|e| CliError::General(e.to_string()))?;

        if !confirm {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Perform the reset
    println!("\nResetting...\n");

    if let Some(ref ws) = workspace {
        if ws.exists() {
            let paths = WorkspacePaths::new(ws.clone());

            for (name, path) in [
                ("store.db", &paths.store_db),
                ("recall.db", &paths.recall_db),
                ("ems.db", &paths.ems_db),
                ("frames.db", &paths.frames_db),
            ] {
                if path.exists() {
                    std::fs::remove_file(path)?;
                    println!("  removed {}", name);
                }
            }

            let mind_memory = paths.mind.join("memory.md");
            if mind_memory.exists() {
                std::fs::remove_file(&mind_memory)?;
                println!("  removed mind/memory.md");
            }

            let mind_self = paths.mind.join("self.md");
            if mind_self.exists() {
                std::fs::remove_file(&mind_self)?;
                println!("  removed mind/self.md");
            }

            let head_dir = ws.join("head");
            if head_dir.exists() {
                std::fs::remove_dir_all(&head_dir)?;
                println!("  removed head/");
            }

            let plugins_path = ws.join("plugins.toml");
            if plugins_path.exists() {
                std::fs::remove_file(&plugins_path)?;
                println!("  removed plugins.toml");
            }

            let workspace_config = ws.join("config.toml");
            if workspace_config.exists() {
                std::fs::remove_file(&workspace_config)?;
                println!("  removed config.toml");
            }
        }
    }

    if reset_config {
        if let Some(ref cp) = config_path {
            if cp.exists() {
                std::fs::remove_file(cp)?;
                println!("  removed ~/.config/abbot/abbot.toml");
            }
        }
    }

    println!("\nReset complete.");
    if reset_config {
        println!("Run 'abbotd run' to regenerate config with defaults.");
    }

    Ok(())
}
