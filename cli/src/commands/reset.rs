//! Reset command - Delete workspace state with confirmation

use std::io::IsTerminal;
use std::path::PathBuf;

use serde_json::json;

use crate::config;
use crate::error::CliError;
use crate::output::{OutputFormat, print_value};

pub fn run(cli_config: Option<PathBuf>, force: bool, reset_config: bool, format: OutputFormat) -> Result<(), CliError> {
    use abbot::runtime::AppConfig;
    use abbot::runtime::app_config::WorkspacePaths;

    config::init_app_config(cli_config.as_deref());

    let workspace = AppConfig::global().workspace_path().ok();
    let config_path = cli_config.or_else(config::default_config_path);

    // Non-interactive guard: require --force when not on a TTY
    if !force && !std::io::stdout().is_terminal() {
        return Err(CliError::General(
            "reset requires a terminal for confirmation; use --force for non-interactive reset".into(),
        ));
    }

    // Show what will be deleted
    println!("This will delete:\n");

    if let Some(ref ws) = workspace {
        if ws.exists() {
            let paths = WorkspacePaths::new(ws.clone());
            for (name, path) in [
                ("store.db (conversation history)", &paths.store_db),
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
        use inquire::Confirm;

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
    let mut removed: Vec<String> = Vec::new();

    if let Some(ref ws) = workspace {
        if ws.exists() {
            let paths = WorkspacePaths::new(ws.clone());

            for (name, path) in [
                ("store.db", &paths.store_db),
                ("ems.db", &paths.ems_db),
                ("frames.db", &paths.frames_db),
            ] {
                if path.exists() {
                    std::fs::remove_file(path)?;
                    removed.push(name.to_string());
                }
            }

            let mind_memory = paths.mind.join("memory.md");
            if mind_memory.exists() {
                std::fs::remove_file(&mind_memory)?;
                removed.push("mind/memory.md".to_string());
            }

            let mind_self = paths.mind.join("self.md");
            if mind_self.exists() {
                std::fs::remove_file(&mind_self)?;
                removed.push("mind/self.md".to_string());
            }

            let head_dir = ws.join("head");
            if head_dir.exists() {
                std::fs::remove_dir_all(&head_dir)?;
                removed.push("head/".to_string());
            }

            let plugins_path = ws.join("plugins.toml");
            if plugins_path.exists() {
                std::fs::remove_file(&plugins_path)?;
                removed.push("plugins.toml".to_string());
            }

            let workspace_config = ws.join("config.toml");
            if workspace_config.exists() {
                std::fs::remove_file(&workspace_config)?;
                removed.push("config.toml".to_string());
            }
        }
    }

    if reset_config {
        if let Some(ref cp) = config_path {
            if cp.exists() {
                std::fs::remove_file(cp)?;
                removed.push("~/.config/abbot/abbot.toml".to_string());
            }
        }
    }

    print_value(&json!({
        "status": "reset_complete",
        "removed": removed,
        "config_reset": reset_config,
    }), format);

    Ok(())
}
