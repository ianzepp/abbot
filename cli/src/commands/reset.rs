//! Reset command - Delete workspace state with confirmation

use std::io::IsTerminal;

use serde_json::json;

use crate::error::CliError;
use crate::output::{OutputFormat, print_value};

pub fn run(force: bool, format: OutputFormat) -> Result<(), CliError> {
    let data_dir = abbot::runtime::app_config::config_dir();

    // Non-interactive guard: require --force when not on a TTY
    if !force && !std::io::stdout().is_terminal() {
        return Err(CliError::General(
            "reset requires a terminal for confirmation; use --force for non-interactive reset"
                .into(),
        ));
    }

    // Show what will be deleted
    println!("This will delete:\n");

    if let Some(ref ws) = data_dir {
        if ws.exists() {
            for (name, sub) in [
                ("store.db (conversation history)", "store.db"),
                ("ems.db (entity storage)", "ems.db"),
                ("frames.db (frame history)", "frames.db"),
            ] {
                let path = ws.join(sub);
                if path.exists() {
                    println!("  {}", name);
                }
            }

            let mind_memory = ws.join("mind").join("memory.md");
            if mind_memory.exists() {
                println!("  mind/memory.md (long-term memory)");
            }

            let mind_self = ws.join("mind").join("self.md");
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
                println!("  config.toml (runtime config)");
            }
        } else {
            println!("  (data directory does not exist: {})", ws.display());
        }
    } else {
        println!("  (could not determine data directory)");
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

    if let Some(ref ws) = data_dir
        && ws.exists()
    {
        for (name, sub) in [
            ("store.db", "store.db"),
            ("ems.db", "ems.db"),
            ("frames.db", "frames.db"),
        ] {
            let path = ws.join(sub);
            if path.exists() {
                std::fs::remove_file(&path)?;
                removed.push(name.to_string());
            }
        }

        let mind_memory = ws.join("mind").join("memory.md");
        if mind_memory.exists() {
            std::fs::remove_file(&mind_memory)?;
            removed.push("mind/memory.md".to_string());
        }

        let mind_self = ws.join("mind").join("self.md");
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

    print_value(
        &json!({
            "status": "reset_complete",
            "removed": removed,
        }),
        format,
    );

    Ok(())
}
