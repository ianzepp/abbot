//! Init command - Interactive first-time setup for Abbot

use std::io::IsTerminal;
use std::path::PathBuf;

use inquire::{Confirm, Password, Select, Text};

use crate::config::{self, load_api_keys, save_api_key};
use crate::error::CliError;

use super::providers::{format_price, refresh_provider};

use abbot::runtime::app_config::atomic_write_file_0600;

pub async fn run(
    cli_config: Option<PathBuf>,
    clean: bool,
    developer: bool,
) -> Result<(), CliError> {
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

    // Trait customization
    let trait_selections = pick_traits()?;

    // User introduction (becomes first memory)
    let intro = Text::new("Tell Abbot a little about yourself (optional):")
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;
    let intro = intro.trim().to_string();

    // Write abbot.toml
    let trait_refs: Vec<(&str, &str)> = trait_selections
        .iter()
        .map(|(c, v)| (c.as_str(), v.as_str()))
        .collect();
    let config_content = config::generate_default_config(&selected_model, &trait_refs, developer);

    atomic_write_file_0600(&config_path, &config_content)?;
    let server_addr = "127.0.0.1:8080";

    // Print summary
    println!();
    println!("Abbot initialized!");
    println!();
    println!("  Config:    {}", config_path.display());
    println!("  Provider:  {}", provider);
    println!("  Model:     {}", selected_model);

    let active: Vec<_> = trait_selections
        .iter()
        .filter(|(_, v)| v != "none")
        .collect();
    if !active.is_empty() {
        let labels: Vec<String> = active.iter().map(|(c, v)| format!("{}/{}", c, v)).collect();
        println!("  Traits:    {}", labels.join(", "));
    }

    // Save user introduction as first memory
    if !intro.is_empty() {
        let ems_path = config::config_dir()
            .map(|d| d.join("ems.db"))
            .ok_or_else(|| CliError::General("could not determine ems.db path".into()))?;

        match abbot::ems::EmsService::open(ems_path.to_str().unwrap_or("ems.db")).await {
            Ok(mut ems) => {
                let memory_prompt = format!("Your user introduced themselves with: {}", intro);
                match ems
                    .insert("memories", &serde_json::json!({ "prompt": memory_prompt }))
                    .await
                {
                    Ok(_) => println!("  Memory:    saved"),
                    Err(e) => eprintln!("  Memory:    failed ({})", e),
                }
            }
            Err(e) => eprintln!("  Memory:    failed to open ems.db ({})", e),
        }
    }

    // Run preflight checks against the newly-written config
    println!();
    println!("Running preflight checks...");
    println!();

    config::init_app_config(cli_config.as_deref());

    let home = dirs::home_dir()
        .ok_or_else(|| CliError::General("could not determine home directory".into()))?;
    let paths = abbot::runtime::app_config::WorkspacePaths::new(home);

    match abbot::runtime::preflight::run_preflight(&paths).await {
        Ok(()) => {}
        Err(e) => eprintln!("Preflight error: {e}"),
    }

    let log_path = abbot::runtime::app_config::config_dir()
        .map(|d| d.join("preflight.log"))
        .filter(|p| p.exists());

    if let Some(path) = log_path {
        if let Ok(content) = std::fs::read_to_string(&path) {
            print!("{}", crate::output::colorize_preflight(&content));
        }
    }

    // Detect and configure coding tool integrations
    configure_integrations(&server_addr)?;

    println!();
    println!("To start:");
    println!("  abbot service install");
    println!("  abbot start");

    Ok(())
}

/// Interactive trait picker. Returns `(category, variant)` pairs for all categories.
/// Categories the user doesn't customize get "none".
fn pick_traits() -> Result<Vec<(String, String)>, CliError> {
    use abbot::runtime::trait_catalog::{self, trait_categories};

    struct TraitOption {
        name: String,
        desc: String,
    }

    impl std::fmt::Display for TraitOption {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            if self.desc.is_empty() {
                write!(f, "{}", self.name)
            } else {
                write!(f, "{:<16} {}", self.name, self.desc)
            }
        }
    }

    let customize = Confirm::new("Customize personality traits?")
        .with_default(false)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    let categories = trait_categories();

    if !customize {
        let result: Vec<(String, String)> = categories
            .iter()
            .map(|(cat, _)| (cat.to_string(), "none".to_string()))
            .collect();
        return Ok(result);
    }

    // Selections: None = "none", Some(idx) = variants[idx]
    let mut selections: Vec<Option<usize>> = vec![None; categories.len()];

    loop {
        // Build the category menu showing current values
        let mut options: Vec<String> = Vec::with_capacity(categories.len() + 1);
        options.push("Done".to_string());

        for (i, &(cat, _)) in categories.iter().enumerate() {
            let current = selections[i]
                .map(|idx| categories[i].1[idx])
                .unwrap_or("none");
            let marker = if selections[i].is_some() {
                " \x1b[32m*\x1b[0m"
            } else {
                ""
            };
            options.push(format!("{:<14} = {}{}", cat, current, marker));
        }

        let choice = Select::new("Pick a trait category to configure:", options)
            .with_page_size(categories.len() + 1)
            .prompt()
            .map_err(|e| CliError::General(e.to_string()))?;

        if choice == "Done" {
            break;
        }

        // Find which category was selected
        let cat_idx = categories
            .iter()
            .position(|&(cat, _)| choice.starts_with(cat));

        let Some(cat_idx) = cat_idx else {
            continue;
        };

        let (cat_name, variants) = categories[cat_idx];

        // Build variant sub-menu with one-liner descriptions from trait files
        let mut variant_options: Vec<TraitOption> = Vec::with_capacity(variants.len() + 1);
        variant_options.push(TraitOption {
            name: "none".to_string(),
            desc: String::new(),
        });
        for &v in variants.iter() {
            let desc = trait_catalog::load_trait(&format!("{}/{}", cat_name, v))
                .and_then(|content| content.lines().nth(2))
                .unwrap_or("")
                .to_string();
            variant_options.push(TraitOption {
                name: v.to_string(),
                desc,
            });
        }

        // Pre-select the current value
        let current_idx = selections[cat_idx].map(|i| i + 1).unwrap_or(0);

        let picked = Select::new(&format!("{}:", cat_name), variant_options)
            .with_starting_cursor(current_idx)
            .prompt()
            .map_err(|e| CliError::General(e.to_string()))?;

        if picked.name == "none" {
            selections[cat_idx] = None;
        } else {
            selections[cat_idx] = variants.iter().position(|&v| v == picked.name);
        }
    }

    let result: Vec<(String, String)> = categories
        .iter()
        .enumerate()
        .map(|(i, &(cat, _))| {
            let variant = selections[i]
                .map(|idx| categories[i].1[idx])
                .unwrap_or("none");
            (cat.to_string(), variant.to_string())
        })
        .collect();
    Ok(result)
}

/// Detect coding tools in PATH and offer to configure them to use Abbot.
fn configure_integrations(server_addr: &str) -> Result<(), CliError> {
    let base_url = format!("http://{}", server_addr);

    let has_claude = std::process::Command::new("which")
        .arg("claude")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let has_opencode = std::process::Command::new("which")
        .arg("opencode")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !has_claude && !has_opencode {
        return Ok(());
    }

    println!();

    if has_claude {
        println!("Claude Code detected. Use `abbot run claude` to launch it through Abbot.");
    }

    if has_opencode {
        configure_opencode(&base_url)?;
    }

    Ok(())
}

/// Offer to configure OpenCode to use Abbot as a provider.
fn configure_opencode(base_url: &str) -> Result<(), CliError> {
    let install = Confirm::new("OpenCode detected. Configure it to use Abbot?")
        .with_default(false)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    if !install {
        return Ok(());
    }

    let config_dir = dirs::home_dir()
        .map(|h| h.join(".config").join("opencode"))
        .ok_or_else(|| CliError::General("could not determine home directory".into()))?;
    let config_path = config_dir.join("opencode.json");

    // Read existing config or start fresh
    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Merge Abbot provider into config.provider
    let provider_block = serde_json::json!({
        "npm": "@ai-sdk/openai-compatible",
        "name": "Abbot",
        "options": {
            "baseURL": format!("{}/v1", base_url)
        },
        "models": {
            "abbot": {
                "name": "Abbot (proxy)"
            }
        }
    });

    config
        .as_object_mut()
        .unwrap()
        .entry("provider")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .unwrap()
        .insert("abbot".to_string(), provider_block);

    std::fs::create_dir_all(&config_dir)?;
    let content =
        serde_json::to_string_pretty(&config).map_err(|e| CliError::General(e.to_string()))?;
    std::fs::write(&config_path, content + "\n")?;

    println!("  Wrote {}", config_path.display());
    println!();

    Ok(())
}
