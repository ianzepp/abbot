//! Init command - Interactive first-time setup for Abbot

use std::io::IsTerminal;
use std::path::PathBuf;

use inquire::autocompletion::{Autocomplete, Replacement};
use inquire::{Confirm, CustomUserError, Password, Select, Text};

use crate::config::{self, load_api_keys, save_api_key};
use crate::error::CliError;

use super::providers::{format_price, refresh_provider};

use abbot::runtime::app_config::atomic_write_file_0600;

// ---------------------------------------------------------------------------
// Provider lookup table
// ---------------------------------------------------------------------------

struct ProviderInfo {
    id: &'static str,
    env_var: &'static str,
    keys_url: &'static str,
    base_url: &'static str,
    default_model: &'static str,
}

const PROVIDERS: &[(&str, ProviderInfo)] = &[
    (
        "anthropic",
        ProviderInfo {
            id: "anthropic",
            env_var: "ANTHROPIC_API_KEY",
            keys_url: "https://console.anthropic.com/settings/keys",
            base_url: "https://api.anthropic.com/v1",
            default_model: "anthropic/claude-sonnet-4-20250514",
        },
    ),
    (
        "openai",
        ProviderInfo {
            id: "openai",
            env_var: "OPENAI_API_KEY",
            keys_url: "https://platform.openai.com/api-keys",
            base_url: "https://api.openai.com/v1",
            default_model: "openai/gpt-4.1",
        },
    ),
    (
        "gemini",
        ProviderInfo {
            id: "gemini",
            env_var: "GEMINI_API_KEY",
            keys_url: "https://aistudio.google.com/apikey",
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
            default_model: "gemini/gemini-2.0-flash",
        },
    ),
    (
        "xai",
        ProviderInfo {
            id: "xai",
            env_var: "XAI_API_KEY",
            keys_url: "https://console.x.ai/team/default/api-keys",
            base_url: "https://api.x.ai/v1",
            default_model: "xai/grok-3-mini",
        },
    ),
    (
        "zai",
        ProviderInfo {
            id: "zai",
            env_var: "ZAI_API_KEY",
            keys_url: "https://z.ai/manage-apikey/apikey-list",
            base_url: "https://api.z.ai/api/paas/v4",
            default_model: "zai/z1-mini",
        },
    ),
    (
        "openrouter",
        ProviderInfo {
            id: "openrouter",
            env_var: "OPENROUTER_API_KEY",
            keys_url: "https://openrouter.ai/settings/keys",
            base_url: "https://openrouter.ai/api/v1",
            default_model: "openrouter/anthropic/claude-sonnet-4",
        },
    ),
    (
        "ollama",
        ProviderInfo {
            id: "ollama",
            env_var: "",
            keys_url: "",
            base_url: "http://localhost:11434/v1",
            default_model: "ollama/llama3.2",
        },
    ),
];

fn lookup_provider(name: &str) -> Result<&'static ProviderInfo, CliError> {
    PROVIDERS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, info)| info)
        .ok_or_else(|| {
            let valid: Vec<&str> = PROVIDERS.iter().map(|(k, _)| *k).collect();
            CliError::General(format!(
                "unknown provider '{}'. Valid: {}",
                name,
                valid.join(", ")
            ))
        })
}

// Display labels for the interactive provider menu (order matches PROVIDERS).
const PROVIDER_LABELS: &[&str] = &[
    "Anthropic (Claude)",
    "OpenAI (GPT)",
    "Google (Gemini)",
    "X.ai (Grok)",
    "Z.ai",
    "OpenRouter (multi-provider)",
    "Ollama (local)",
];

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run(
    cli_config: Option<PathBuf>,
    clean: bool,
    developer: bool,
    cli_provider: Option<String>,
    cli_model: Option<String>,
    accept_defaults: bool,
) -> Result<(), CliError> {
    // Validate flags
    if accept_defaults && cli_provider.is_none() {
        return Err(CliError::General(
            "--provider is required with --accept-defaults".into(),
        ));
    }

    // Terminal guard — interactive prompts require a TTY (unless accepting defaults)
    if !accept_defaults && !std::io::stdout().is_terminal() {
        return Err(CliError::General(
            "abbot init requires a terminal for interactive input (or use --accept-defaults)"
                .into(),
        ));
    }

    // Safety warning — this program executes LLM-generated commands on your system
    if !accept_defaults {
        println!();
        println!("WARNING: Abbot is an autonomous AI agent that executes commands on");
        println!("your system. It can create, modify, and delete files. While it operates");
        println!("within a sandbox by default, misconfiguration or bugs may cause");
        println!("unintended changes to your system.");
        println!();
        let proceed = Confirm::new("Do you understand the risks and want to continue?")
            .with_default(false)
            .prompt()
            .map_err(|e| CliError::General(e.to_string()))?;
        if !proceed {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Home directory — ask immediately after the safety warning
    let mounts = if accept_defaults {
        Vec::new()
    } else {
        prompt_home_directory()?
    };

    // Resolve config path
    let config_path = cli_config
        .clone()
        .or_else(config::default_config_path)
        .ok_or(CliError::General("could not determine config path".into()))?;

    // --clean with --accept-defaults: wipe immediately, then re-create dir
    if clean && accept_defaults {
        if let Some(dir) = config_path.parent().filter(|d| d.exists()) {
            std::fs::remove_dir_all(dir)?;
            std::fs::create_dir_all(dir)?;
            println!("Cleaned {}", dir.display());
        }
    } else if !clean && config_path.exists() && !accept_defaults {
        let overwrite = Confirm::new("Config already exists. Overwrite?")
            .with_default(false)
            .prompt()
            .map_err(|e| CliError::General(e.to_string()))?;
        if !overwrite {
            // Even when keeping existing config, ensure API keys from the
            // environment are persisted to keys.env so the launchd-started
            // daemon can find them.
            sync_env_keys_to_file(&config_path);
            println!("Keeping existing config.");
            println!("To change models, run: abbot providers use <model>");
            return Ok(());
        }
    }

    // Load existing API keys from keys.env into process env (before --clean wipes them)
    load_api_keys();

    // --- Collect configuration ---
    // Track the provider's env var name so we can re-save the API key after --clean wipe.
    #[allow(clippy::type_complexity)]
    let (
        provider_name,
        provider_env_var,
        selected_model,
        trait_selections,
        tick_interval,
        intro,
        want_install,
        want_start,
    ): (
        String,
        &str,
        String,
        Vec<(String, String)>,
        u64,
        String,
        bool,
        bool,
    ) = if accept_defaults {
        let provider_info = lookup_provider(cli_provider.as_ref().unwrap())?;

        let selected_model = match cli_model {
            Some(m) if m.contains('/') => m,
            Some(m) => format!("{}/{}", provider_info.id, m),
            None => provider_info.default_model.to_string(),
        };
        (
            provider_info.id.to_string(),
            provider_info.env_var,
            selected_model,
            default_traits(),
            1800,
            String::new(),
            false,
            false,
        )
    } else {
        loop {
            // --- Provider selection ---
            let provider_info = select_provider(cli_provider.as_deref())?;
            let provider = provider_info.id;

            // --- API key handling ---
            // prompt_api_key saves to keys.env AND sets the env var. The env var
            // survives the --clean wipe; we re-persist it to keys.env afterward.
            if !provider_info.env_var.is_empty() {
                prompt_api_key(provider_info.env_var, provider_info.keys_url, provider)?;
            }

            // --- Model selection ---
            let selected_model = if let Some(ref m) = cli_model {
                if m.contains('/') {
                    m.clone()
                } else {
                    format!("{}/{}", provider, m)
                }
            } else {
                select_model_interactive(provider, provider_info.default_model).await?
            };

            // --- API verification ---
            verify_api(provider_info, &selected_model).await;

            // --- Trait customization ---
            let trait_selections = pick_traits()?;

            // --- Wake cadence ---
            let tick_interval = pick_wake_cadence()?;

            // --- User introduction ---
            let intro = prompt_introduction()?;

            // --- Service install/start intent ---
            let (want_install, want_start) = prompt_service_intent()?;

            // --- Summary + confirmation ---
            if confirm_summary(
                provider,
                &selected_model,
                &trait_selections,
                tick_interval,
                &mounts,
                &intro,
                want_install,
                want_start,
            )? {
                break (
                    provider.to_string(),
                    provider_info.env_var,
                    selected_model,
                    trait_selections,
                    tick_interval,
                    intro,
                    want_install,
                    want_start,
                );
            }

            println!("\nStarting over...\n");
        }
    };

    // --- Deferred --clean wipe (interactive mode) ---
    if clean
        && !accept_defaults
        && let Some(dir) = config_path.parent().filter(|d| d.exists())
    {
        std::fs::remove_dir_all(dir)?;
        std::fs::create_dir_all(dir)?;
        println!("Cleaned {}", dir.display());
    }

    // Ensure config parent directory exists (needed after --clean wipe)
    if let Some(dir) = config_path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    // Re-save API key from env to keys.env (survives --clean wipe via process env)
    if !provider_env_var.is_empty()
        && let Ok(key) = std::env::var(provider_env_var)
        && !key.is_empty()
    {
        let _ = save_api_key(provider_env_var, &key);
    }

    // --- Write abbot.toml ---
    let trait_refs: Vec<(&str, &str)> = trait_selections
        .iter()
        .map(|(c, v)| (c.as_str(), v.as_str()))
        .collect();
    let mount_refs: Vec<(&str, &str)> = mounts
        .iter()
        .map(|(p, h)| (p.as_str(), h.as_str()))
        .collect();
    let config_content = config::generate_default_config(
        &selected_model,
        &trait_refs,
        developer,
        tick_interval,
        &mount_refs,
    );

    atomic_write_file_0600(&config_path, &config_content)?;

    // --- Saved summary ---
    println!();
    println!("Abbot initialized!");
    println!("  Config:    {}", config_path.display());
    println!("  Provider:  {}", provider_name);
    println!("  Model:     {}", selected_model);

    // --- Save user introduction as memory ---
    if !intro.is_empty() {
        let ems_path = config_path
            .parent()
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

    // --- Preflight + integrations (skip in non-interactive mode) ---
    if !accept_defaults {
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

        if let Some(path) = log_path
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            print!("{}", crate::output::colorize_preflight(&content));
        }

        configure_integrations();

        // --- Execute service install/start ---
        if want_install {
            super::service::run(
                super::service::ServiceAction::Install,
                crate::output::OutputFormat::Pretty,
            )
            .await?;

            if want_start {
                super::service::start_service(crate::output::OutputFormat::Pretty).await?;
            } else {
                println!();
                println!("To start later: abbot start");
            }
        } else {
            println!();
            println!("To install later:");
            println!("  abbot service install");
            println!("  abbot start");
        }

        println!();
        println!("To add more mount points: abbot mount <path>");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Interactive provider selection
// ---------------------------------------------------------------------------

fn select_provider(cli_provider: Option<&str>) -> Result<&'static ProviderInfo, CliError> {
    if let Some(name) = cli_provider {
        return lookup_provider(name);
    }

    let labels = PROVIDER_LABELS.to_vec();
    let choice = Select::new("Select a provider:", labels)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;
    let idx = PROVIDER_LABELS.iter().position(|&l| l == choice).unwrap();
    Ok(&PROVIDERS[idx].1)
}

// ---------------------------------------------------------------------------
// Interactive introduction prompt
// ---------------------------------------------------------------------------

fn prompt_introduction() -> Result<String, CliError> {
    Text::new("Tell Abbot a little about yourself (optional):")
        .prompt()
        .map(|s| s.trim().to_string())
        .map_err(|e| CliError::General(e.to_string()))
}

// ---------------------------------------------------------------------------
// Interactive service install/start intent
// ---------------------------------------------------------------------------

fn prompt_service_intent() -> Result<(bool, bool), CliError> {
    let want_install = Confirm::new("Install as a system service?")
        .with_default(true)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    let want_start = if want_install {
        Confirm::new("Start the service now?")
            .with_default(true)
            .prompt()
            .map_err(|e| CliError::General(e.to_string()))?
    } else {
        false
    };

    Ok((want_install, want_start))
}

// ---------------------------------------------------------------------------
// Configuration summary + confirmation
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn confirm_summary(
    provider: &str,
    model: &str,
    traits: &[(String, String)],
    tick_interval: u64,
    mounts: &[(String, String)],
    intro: &str,
    want_install: bool,
    want_start: bool,
) -> Result<bool, CliError> {
    println!();
    println!("Configuration summary:");
    println!("  Provider:  {}", provider);
    println!("  Model:     {}", model);

    let active_traits: Vec<_> = traits.iter().filter(|(_, v)| v != "none").collect();
    if active_traits.is_empty() {
        println!("  Traits:    (none)");
    } else {
        let labels: Vec<String> = active_traits
            .iter()
            .map(|(c, v)| format!("{}/{}", c, v))
            .collect();
        println!("  Traits:    {}", labels.join(", "));
    }

    let cadence_label = match tick_interval {
        60 => "Very Fast (60s)",
        300 => "Fast (5m)",
        1800 => "Normal (30m)",
        7200 => "Slow (2hr)",
        0 => "On Demand Only",
        other => &format!("{}s", other),
    };
    println!("  Cadence:   {}", cadence_label);

    for (prefix, host) in mounts {
        println!("  Mount:     {} -> {}", prefix, host);
    }

    if !intro.is_empty() {
        println!("  Intro:     {}", intro);
    }

    if want_install && want_start {
        println!("  Service:   install + start");
    } else if want_install {
        println!("  Service:   install only");
    } else {
        println!("  Service:   skip");
    }

    println!();
    Confirm::new("Save this configuration?")
        .with_default(true)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))
}

// ---------------------------------------------------------------------------
// File path autocompleter
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FilePathCompleter;

impl FilePathCompleter {
    /// Expand `~` prefix to home directory.
    fn expand(input: &str) -> String {
        if input.starts_with('~')
            && let Some(home) = dirs::home_dir()
        {
            return input.replacen('~', &home.to_string_lossy(), 1);
        }
        input.to_string()
    }

    /// List directory entries that match the current input prefix.
    /// Skips dot-entries unless the basename already starts with '.'.
    fn completions(input: &str) -> Vec<String> {
        let expanded = Self::expand(input);
        let path = std::path::Path::new(&expanded);

        // Determine the directory to scan and the partial basename to match
        let (dir, prefix) = if path.is_dir() && expanded.ends_with('/') {
            (path.to_path_buf(), String::new())
        } else {
            let dir = path.parent().unwrap_or(std::path::Path::new("/"));
            let prefix = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            (dir.to_path_buf(), prefix)
        };

        let show_hidden = prefix.starts_with('.');

        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };

        let mut results: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if !show_hidden && name.starts_with('.') {
                    return None;
                }
                if !name.starts_with(&prefix) {
                    return None;
                }
                // Build the full suggestion, preserving ~ in output
                let full = dir.join(&name);
                let display = if input.starts_with('~') {
                    if let Some(home) = dirs::home_dir() {
                        let home_str = home.to_string_lossy();
                        full.to_string_lossy().replacen(home_str.as_ref(), "~", 1)
                    } else {
                        full.to_string_lossy().to_string()
                    }
                } else {
                    full.to_string_lossy().to_string()
                };
                Some(format!("{}/", display))
            })
            .collect();

        results.sort();
        results
    }

    /// Longest common prefix across all suggestions.
    fn longest_common_prefix(suggestions: &[String]) -> Option<String> {
        let first = suggestions.first()?;
        let mut prefix = first.clone();
        for s in &suggestions[1..] {
            while !s.starts_with(&prefix) {
                prefix.pop();
            }
        }
        if prefix.is_empty() {
            None
        } else {
            Some(prefix)
        }
    }
}

impl Autocomplete for FilePathCompleter {
    fn get_suggestions(&mut self, input: &str) -> Result<Vec<String>, CustomUserError> {
        Ok(Self::completions(input))
    }

    fn get_completion(
        &mut self,
        input: &str,
        highlighted: Option<String>,
    ) -> Result<Replacement, CustomUserError> {
        if let Some(selected) = highlighted {
            return Ok(Some(selected));
        }
        let suggestions = Self::completions(input);
        Ok(Self::longest_common_prefix(&suggestions))
    }
}

// ---------------------------------------------------------------------------
// Home directory prompt
// ---------------------------------------------------------------------------

/// Prompt for Abbot's primary home directory on the host filesystem.
/// First offers full home directory access, then falls back to a specific path.
/// Returns a single-element vec with `("/home", host_path)`.
fn prompt_home_directory() -> Result<Vec<(String, String)>, CliError> {
    let use_home = Confirm::new("Give Abbot access to your entire home directory?")
        .with_default(false)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    if use_home {
        let home = dirs::home_dir()
            .ok_or_else(|| CliError::General("could not determine home directory".into()))?;
        return Ok(vec![(
            "/home".to_string(),
            home.to_string_lossy().to_string(),
        )]);
    }

    let raw_path = Text::new("What is Abbot's primary home directory?")
        .with_autocomplete(FilePathCompleter)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    let raw_path = raw_path.trim();

    // Empty input → create ~/.abbot/sandbox/home as a regular directory
    if raw_path.is_empty() {
        let sandbox_home = config::config_dir()
            .ok_or_else(|| CliError::General("could not determine config directory".into()))?
            .join("sandbox")
            .join("home");
        std::fs::create_dir_all(&sandbox_home)?;
        return Ok(vec![(
            "/home".to_string(),
            sandbox_home.to_string_lossy().to_string(),
        )]);
    }

    // Expand ~ to home directory
    let expanded = if raw_path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            raw_path.replacen('~', &home.to_string_lossy(), 1)
        } else {
            raw_path.to_string()
        }
    } else {
        raw_path.to_string()
    };

    let path = std::path::Path::new(&expanded);
    if !path.is_dir() {
        return Err(CliError::General(format!(
            "Not a valid directory: {}",
            expanded
        )));
    }

    Ok(vec![("/home".to_string(), expanded)])
}

// ---------------------------------------------------------------------------
// API verification
// ---------------------------------------------------------------------------

/// Send a quick "what is 2+2" chat completion to verify the API key and model work.
async fn verify_api(provider: &ProviderInfo, model: &str) {
    print!("Verifying API connection... ");

    // Strip provider prefix from model ID (e.g. "anthropic/claude-sonnet-4-..." -> "claude-sonnet-4-...")
    let model_id = model.find('/').map(|i| &model[i + 1..]).unwrap_or(model);

    let api_key = if !provider.env_var.is_empty() {
        std::env::var(provider.env_var)
            .ok()
            .filter(|v| !v.is_empty())
    } else {
        None
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    let base = provider.base_url.trim_end_matches('/');

    let result = if provider.id == "anthropic" {
        verify_anthropic(&client, base, api_key.as_deref(), model_id).await
    } else {
        verify_openai_compat(&client, base, api_key.as_deref(), model_id).await
    };

    match result {
        Ok(answer) => println!("ok ({})", answer.trim()),
        Err(e) => println!("FAILED: {}", e),
    }
}

async fn verify_anthropic(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
) -> Result<String, String> {
    let key = api_key.ok_or("API key not set")?;
    let url = format!("{}/messages", base_url);

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 32,
        "messages": [{"role": "user", "content": "What is 2+2? Reply with just the number."}]
    });

    let resp = client
        .post(&url)
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {} - {}", status, text));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| format!("{e}"))?;
    let text = json["content"][0]["text"]
        .as_str()
        .unwrap_or("no response")
        .to_string();
    Ok(text)
}

async fn verify_openai_compat(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url);

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 32,
        "messages": [{"role": "user", "content": "What is 2+2? Reply with just the number."}]
    });

    let mut req = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&body);

    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let resp = req.send().await.map_err(|e| format!("{e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        // Some completion-only models reject /chat/completions. For init verification,
        // retry with /completions.
        if text.contains("not a chat model")
            && (text.contains("v1/completions") || text.contains("/completions"))
        {
            let url2 = format!("{}/completions", base_url);
            let body2 = serde_json::json!({
                "model": model,
                "max_tokens": 32,
                "prompt": "What is 2+2? Reply with just the number.\n"
            });

            let mut req2 = client
                .post(&url2)
                .header("content-type", "application/json")
                .json(&body2);

            if let Some(key) = api_key {
                req2 = req2.header("Authorization", format!("Bearer {key}"));
            }

            let resp2 = req2.send().await.map_err(|e| format!("{e}"))?;
            if !resp2.status().is_success() {
                let s2 = resp2.status();
                let t2 = resp2.text().await.unwrap_or_default();
                return Err(format!("HTTP {} - {}", s2, t2));
            }

            let json: serde_json::Value = resp2.json().await.map_err(|e| format!("{e}"))?;
            let text = json["choices"][0]["text"]
                .as_str()
                .unwrap_or("no response")
                .to_string();
            return Ok(text);
        }

        // Some newer OpenAI chat models reject max_tokens in favor of max_completion_tokens.
        if text.contains("Unsupported parameter")
            && text.contains("max_tokens")
            && text.contains("max_completion_tokens")
        {
            let url2 = format!("{}/chat/completions", base_url);
            let body2 = serde_json::json!({
                "model": model,
                "max_completion_tokens": 32,
                "messages": [{"role": "user", "content": "What is 2+2? Reply with just the number."}]
            });

            let mut req2 = client
                .post(&url2)
                .header("content-type", "application/json")
                .json(&body2);

            if let Some(key) = api_key {
                req2 = req2.header("Authorization", format!("Bearer {key}"));
            }

            let resp2 = req2.send().await.map_err(|e| format!("{e}"))?;
            if !resp2.status().is_success() {
                let s2 = resp2.status();
                let t2 = resp2.text().await.unwrap_or_default();
                return Err(format!("HTTP {} - {}", s2, t2));
            }

            let json: serde_json::Value = resp2.json().await.map_err(|e| format!("{e}"))?;
            let text = json["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("no response")
                .to_string();
            return Ok(text);
        }

        return Err(format!("HTTP {} - {}", status, text));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| format!("{e}"))?;
    let text = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("no response")
        .to_string();
    Ok(text)
}

// ---------------------------------------------------------------------------
// Interactive model selection
// ---------------------------------------------------------------------------

async fn select_model_interactive(provider: &str, default_model: &str) -> Result<String, CliError> {
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
                Ok(default_model.to_string())
            } else {
                let selected = Select::new("Select model:", options)
                    .prompt()
                    .map_err(|e| CliError::General(e.to_string()))?;

                if selected.id.starts_with(&format!("{}/", provider)) {
                    Ok(selected.id)
                } else {
                    Ok(format!("{}/{}", provider, selected.id))
                }
            }
        }
        Err(e) => {
            println!("failed ({})", e);
            println!("Using default model.");
            Ok(default_model.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Default traits (all "none")
// ---------------------------------------------------------------------------

fn default_traits() -> Vec<(String, String)> {
    use abbot::runtime::trait_catalog::trait_categories;

    trait_categories()
        .iter()
        .map(|&(cat, _)| (cat.to_string(), "none".to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Interactive trait picker
// ---------------------------------------------------------------------------

/// Interactive trait picker. Returns `(category, variant)` pairs for all categories.
/// Categories the user doesn't customize get "none".
fn pick_traits() -> Result<Vec<(String, String)>, CliError> {
    use abbot::runtime::trait_catalog::{self, personality_presets, trait_categories};

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

    let categories = trait_categories();
    let presets = personality_presets();

    // --- Step 1: Personality preset ---
    struct PresetOption {
        name: String,
        desc: String,
    }

    impl std::fmt::Display for PresetOption {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{:<16} {}", self.name, self.desc)
        }
    }

    let mut preset_options: Vec<PresetOption> = presets
        .iter()
        .map(|&(name, desc, _)| PresetOption {
            name: name.to_string(),
            desc: desc.to_string(),
        })
        .collect();
    preset_options.push(PresetOption {
        name: "None".to_string(),
        desc: "Start with a blank slate".to_string(),
    });

    let picked_preset = Select::new("Choose a personality preset:", preset_options)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    // Seed selections from the chosen preset
    let mut selections: Vec<Option<usize>> = vec![None; categories.len()];

    if let Some(&(_, _, preset_traits)) = presets
        .iter()
        .find(|&&(name, _, _)| name == picked_preset.name)
    {
        for &(cat, variant) in preset_traits {
            if let Some(cat_idx) = categories.iter().position(|&(c, _)| c == cat)
                && let Some(var_idx) = categories[cat_idx].1.iter().position(|&v| v == variant)
            {
                selections[cat_idx] = Some(var_idx);
            }
        }
    }

    // --- Step 2: Customize further? ---
    let customize = Confirm::new("Customize individual traits?")
        .with_default(false)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    if !customize {
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
        return Ok(result);
    }

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

/// Interactive wake cadence picker. Returns the tick_interval in seconds.
fn pick_wake_cadence() -> Result<u64, CliError> {
    let options = vec![
        "Very Fast (60s)",
        "Fast (5m)",
        "Normal (30m)",
        "Slow (2hr)",
        "On Demand Only",
    ];

    let choice = Select::new("How often should Abbot wake up for work?", options)
        .with_starting_cursor(2)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    let secs = match choice {
        "Very Fast (60s)" => 60,
        "Fast (5m)" => 300,
        "Normal (30m)" => 1800,
        "Slow (2hr)" => 7200,
        "On Demand Only" => 0,
        _ => unreachable!(),
    };

    Ok(secs)
}

/// Prompt the user for an API key using a 3-step flow:
/// 1. If the key is in the environment, offer to reuse it
/// 2. Otherwise, offer to open the provider's key management page
/// 3. Otherwise, ask the user to paste the key manually
///
/// Returns `true` if a key was saved, `false` if skipped.
fn prompt_api_key(env_var: &str, keys_url: &str, provider: &str) -> Result<bool, CliError> {
    // Step 1: check if the key is already in the environment
    let existing = std::env::var(env_var).ok().filter(|v| !v.is_empty());

    if let Some(existing_key) = existing {
        println!("{} found in environment.", env_var);
        let reuse = Confirm::new("Use this key?")
            .with_default(true)
            .prompt()
            .map_err(|e| CliError::General(e.to_string()))?;

        if reuse {
            save_api_key(env_var, &existing_key)?;
            println!("Saved to ~/.abbot/keys.env");
            return Ok(true);
        }
    }

    // Step 2: offer to open the provider's API key page
    let open_browser = Confirm::new(&format!("Open {} to create an API key?", keys_url))
        .with_default(true)
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    if open_browser && let Err(e) = open::that(keys_url) {
        eprintln!("Could not open browser: {}", e);
        println!("Visit: {}", keys_url);
    }

    // Step 3: ask the user to paste the key
    let api_key = Password::new(&format!("Paste your {} key:", env_var))
        .without_confirmation()
        .prompt()
        .map_err(|e| CliError::General(e.to_string()))?;

    if api_key.is_empty() {
        println!(
            "Skipping — add later with: abbot providers add {}",
            provider
        );
        return Ok(false);
    }

    save_api_key(env_var, &api_key)?;
    unsafe {
        std::env::set_var(env_var, &api_key);
    }
    println!("Saved to ~/.abbot/keys.env");
    Ok(true)
}

/// Read the existing config and persist any provider API keys found in the
/// current environment to `~/.abbot/keys.env`. This ensures the daemon
/// (launched via launchd, which lacks shell env) can resolve the keys.
fn sync_env_keys_to_file(config_path: &std::path::Path) {
    let content = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let config: abbot::runtime::app_config::AppConfig = match toml::from_str(&content) {
        Ok(c) => c,
        Err(_) => return,
    };

    for provider in config.providers.values() {
        if let Some(ref env_var) = provider.api_key_env
            && let Ok(val) = std::env::var(env_var)
            && !val.is_empty()
        {
            let _ = save_api_key(env_var, &val);
        }
    }
}

/// Detect coding tools in PATH and print `abbot run <tool>` hints.
fn configure_integrations() {
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
        return;
    }

    println!();

    if has_claude {
        println!("Claude Code detected. Use `abbot run claude` to launch it through Abbot.");
    }

    if has_opencode {
        println!("OpenCode detected. Use `abbot run opencode` to launch it through Abbot.");
    }
}
