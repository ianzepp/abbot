//! Init command - Interactive first-time setup for Abbot

use std::io::IsTerminal;
use std::path::PathBuf;

use inquire::{Confirm, Password, Select};

use crate::config::{self, load_api_keys, save_api_key};
use crate::error::CliError;

use super::providers::{format_price, refresh_provider};

use abbot::runtime::app_config::atomic_write_file_0600;

pub async fn run(cli_config: Option<PathBuf>, clean: bool) -> Result<(), CliError> {
    // Terminal guard — interactive prompts require a TTY
    if !std::io::stdout().is_terminal() {
        return Err(CliError::General(
            "abbot init requires a terminal for interactive input".into(),
        ));
    }

    // Resolve config path
    let config_path = cli_config
        .clone()
        .or_else(config::default_config_path)
        .ok_or(CliError::General("could not determine config path".into()))?;

    // --clean: wipe ~/.abbot/ entirely and start fresh
    if clean {
        if let Some(dir) = config::config_dir() {
            if dir.exists() {
                let confirm = Confirm::new("This will delete everything in ~/.abbot/. Continue?")
                    .with_default(false)
                    .prompt()
                    .map_err(|e| CliError::General(e.to_string()))?;

                if !confirm {
                    println!("Aborted.");
                    return Ok(());
                }

                std::fs::remove_dir_all(&dir)?;
                println!("Removed {}", dir.display());
            }
        }
    } else if config_path.exists() {
        // Normal overwrite check (only when not --clean)
        let overwrite = Confirm::new("Config already exists. Overwrite?")
            .with_default(false)
            .prompt()
            .map_err(|e| CliError::General(e.to_string()))?;

        if !overwrite {
            println!("Keeping existing config.");
            println!("To change models, run: abbot providers use <model>");
            return Ok(());
        }
    }

    // Create ~/.abbot/ directory
    if let Some(dir) = config::config_dir() {
        std::fs::create_dir_all(&dir)?;
    }

    // Load existing API keys from keys.env
    load_api_keys();

    // Provider selection
    let providers = vec![
        "Anthropic (Claude)",
        "OpenAI (GPT)",
        "OpenRouter (multi-provider)",
        "Ollama (local)",
    ];

    let provider_choice = Select::new("Select a provider:", providers)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    let (provider, env_var, needs_key, default_model) = match provider_choice {
        "Anthropic (Claude)" => (
            "anthropic",
            "ANTHROPIC_API_KEY",
            true,
            "anthropic/claude-sonnet-4-20250514",
        ),
        "OpenAI (GPT)" => ("openai", "OPENAI_API_KEY", true, "openai/gpt-4.1"),
        "OpenRouter (multi-provider)" => (
            "openrouter",
            "OPENROUTER_API_KEY",
            true,
            "openrouter/anthropic/claude-sonnet-4",
        ),
        "Ollama (local)" => ("ollama", "", false, "ollama/llama3.2"),
        _ => unreachable!(),
    };

    // API key handling
    let mut have_key = false;
    if needs_key {
        let existing = std::env::var(env_var).ok().filter(|v| !v.is_empty());

        if let Some(existing_key) = existing {
            println!("{} is already configured.", env_var);
            let reuse = Confirm::new("Use existing key?")
                .with_default(true)
                .prompt()
                .map_err(|e| CliError::General(e.to_string()))?;

            if reuse {
                // Persist the existing environment key so service launches (launchd/systemd)
                // can load it even when shell init files aren't run.
                save_api_key(env_var, &existing_key)?;
                println!("Saved to ~/.abbot/keys.env");
                have_key = true;
            } else {
                let api_key = Password::new(&format!("{}:", env_var))
                    .without_confirmation()
                    .prompt()
                    .map_err(|e| CliError::General(e.to_string()))?;

                if !api_key.is_empty() {
                    save_api_key(env_var, &api_key)?;
                    unsafe {
                        std::env::set_var(env_var, &api_key);
                    }
                    println!("Saved to ~/.abbot/keys.env");
                    have_key = true;
                }
            }
        } else {
            let api_key = Password::new(&format!("{}:", env_var))
                .without_confirmation()
                .prompt()
                .map_err(|e| CliError::General(e.to_string()))?;

            if api_key.is_empty() {
                println!(
                    "Skipping — add later with: abbot providers add {}",
                    provider
                );
            } else {
                save_api_key(env_var, &api_key)?;
                unsafe {
                    std::env::set_var(env_var, &api_key);
                }
                println!("Saved to ~/.abbot/keys.env");
                have_key = true;
            }
        }
    } else {
        have_key = true; // Ollama doesn't need a key
    }

    // Fetch models and select
    let selected_model = if have_key {
        print!("Fetching models... ");

        #[derive(Clone)]
        struct ModelOption {
            id: String,
            display: String,
        }

        impl std::fmt::Display for ModelOption {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.display)
            }
        }

        match refresh_provider(provider).await {
            Ok(cache) => {
                println!("{} models cached", cache.models.len());

                let options: Vec<ModelOption> = cache
                    .models
                    .iter()
                    .take(20)
                    .map(|m| {
                        let price_info = format!(
                            "{} / {}",
                            format_price(m.input_cost),
                            format_price(m.output_cost)
                        );
                        let ctx = m
                            .context_window
                            .map(|c| format!("{}k", c / 1000))
                            .unwrap_or_else(|| "-".to_string());
                        ModelOption {
                            id: m.id.clone(),
                            display: format!("{:<45} {:>12}  ctx:{}", m.id, price_info, ctx),
                        }
                    })
                    .collect();

                if options.is_empty() {
                    default_model.to_string()
                } else {
                    let selected = Select::new("Select model:", options)
                        .prompt()
                        .map_err(|e| CliError::General(e.to_string()))?;

                    // Prefix with provider if needed
                    if selected.id.starts_with(&format!("{}/", provider)) {
                        selected.id
                    } else {
                        format!("{}/{}", provider, selected.id)
                    }
                }
            }
            Err(e) => {
                println!("failed ({})", e);
                println!("Using default model.");
                default_model.to_string()
            }
        }
    } else {
        default_model.to_string()
    };

    // Write abbot.toml
    let config_content = config::generate_default_config(&selected_model);

    atomic_write_file_0600(&config_path, &config_content)?;

    // Print summary
    println!();
    println!("Abbot initialized!");
    println!();
    println!("  Config:    {}", config_path.display());
    println!("  Provider:  {}", provider);
    println!("  Model:     {}", selected_model);
    println!();
    println!("To start:");
    println!("  abbot service install");
    println!("  abbot start");

    Ok(())
}
