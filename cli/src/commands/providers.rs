//! Providers command - Manage model providers (refresh, list, test, login, add, remove, use)

use clap::Subcommand;

use crate::config::{
    self, CachedModel, ProviderCache, load_api_keys, load_provider_cache, remove_api_key,
    save_api_key, save_provider_cache, update_config_model,
};
use crate::error::CliError;

#[derive(Debug, Subcommand, Clone)]
pub enum ProvidersAction {
    /// Refresh model lists from all providers
    Refresh,
    /// List cached providers and model counts
    List,
    /// Show models from a cached provider
    Models {
        /// Provider name: anthropic, openai, openrouter, ollama
        provider: String,
        /// Max number of models to show (default: 20)
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Open browser to get API key and configure provider
    Login {
        /// Provider name: anthropic, openai, openrouter
        provider: String,
    },
    /// Add/configure a provider with API key (no browser)
    Add {
        /// Provider name: anthropic, openai, openrouter, ollama
        provider: String,
    },
    /// Remove a provider's API key
    Remove {
        /// Provider name to remove
        provider: String,
    },
    /// Test API keys and connectivity for all providers
    Test,
    /// Switch all model configs to use a specific provider/model
    Use {
        /// Model ID in provider/model format (e.g., anthropic/claude-3-5-haiku-latest)
        model: String,
    },
}

pub async fn run(action: ProvidersAction) -> Result<(), CliError> {
    load_api_keys();

    match action {
        ProvidersAction::Refresh => {
            let dir = config::providers_dir()
                .ok_or(CliError::General("could not determine providers directory".into()))?;
            std::fs::create_dir_all(&dir)?;
            println!("Refreshing provider model lists...\n");

            print!("openrouter: ");
            match refresh_provider("openrouter").await {
                Ok(cache) => println!("{} models", cache.models.len()),
                Err(e) => println!("error - {}", e),
            }

            print!("anthropic:  ");
            if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                match refresh_provider("anthropic").await {
                    Ok(cache) => println!("{} models", cache.models.len()),
                    Err(e) => println!("error - {}", e),
                }
            } else {
                println!("skipped (ANTHROPIC_API_KEY not set)");
            }

            print!("openai:     ");
            if std::env::var("OPENAI_API_KEY").is_ok() {
                match refresh_provider("openai").await {
                    Ok(cache) => println!("{} models", cache.models.len()),
                    Err(e) => println!("error - {}", e),
                }
            } else {
                println!("skipped (OPENAI_API_KEY not set)");
            }

            print!("ollama:     ");
            match refresh_provider("ollama").await {
                Ok(cache) => {
                    if cache.models.is_empty() {
                        println!("no models (is ollama running?)");
                    } else {
                        println!("{} models", cache.models.len());
                    }
                }
                Err(e) => println!("error - {}", e),
            }

            println!("\nCached to: {}", dir.display());
        }

        ProvidersAction::List => {
            let dir = config::providers_dir()
                .ok_or(CliError::General("could not determine providers directory".into()))?;

            if !dir.exists() {
                println!("No providers cached. Run: abbot providers refresh");
                return Ok(());
            }

            println!("Cached providers:\n");

            for provider in ["openrouter", "anthropic", "openai", "ollama"] {
                if let Some(cache) = load_provider_cache(provider) {
                    println!(
                        "  {:<12} {:>4} models  ({})",
                        provider,
                        cache.models.len(),
                        cache.fetched_at
                    );
                }
            }

            println!("\nCache directory: {}", dir.display());
        }

        ProvidersAction::Models { provider, limit } => {
            let provider = provider.to_lowercase();

            match load_provider_cache(&provider) {
                Some(cache) => {
                    println!("Models from {} ({}):\n", provider, cache.fetched_at);
                    for m in cache.models.iter().take(limit) {
                        let price_info = format!(
                            "{} / {}",
                            format_price(m.input_cost),
                            format_price(m.output_cost)
                        );
                        let ctx = m
                            .context_window
                            .map(|c| format!("{}k", c / 1000))
                            .unwrap_or_else(|| "-".to_string());
                        let name = m.name.as_deref().unwrap_or("");
                        if name.is_empty() {
                            println!("  {:<45} {:>12}  ctx:{}", m.id, price_info, ctx);
                        } else {
                            println!("  {:<45} {:>12}  ctx:{}", m.id, price_info, ctx);
                            println!("    {}", name);
                        }
                    }
                    if cache.models.len() > limit {
                        println!(
                            "\n  ... and {} more (use --limit to show more)",
                            cache.models.len() - limit
                        );
                    }
                    println!("\nUse: abbot providers use {}/{}", provider, "<model>");
                }
                None => {
                    println!(
                        "No cache for '{}'. Run: abbot providers refresh",
                        provider
                    );
                }
            }
        }

        ProvidersAction::Login { provider } => {
            use inquire::Password;

            let provider = provider.to_lowercase();

            let (env_var, key_url, default_model) = match provider.as_str() {
                "anthropic" => (
                    "ANTHROPIC_API_KEY",
                    "https://console.anthropic.com/settings/keys",
                    "anthropic/claude-3-5-haiku-latest",
                ),
                "openai" => (
                    "OPENAI_API_KEY",
                    "https://platform.openai.com/api-keys",
                    "openai/gpt-4o-mini",
                ),
                "openrouter" => (
                    "OPENROUTER_API_KEY",
                    "https://openrouter.ai/settings/keys",
                    "openrouter/anthropic/claude-sonnet-4",
                ),
                "ollama" => {
                    println!("Ollama runs locally and doesn't need an API key.");
                    return Ok(());
                }
                _ => {
                    println!("Unknown provider: {}", provider);
                    println!("Available: anthropic, openai, openrouter");
                    return Ok(());
                }
            };

            println!("Opening {} to create an API key...\n", key_url);

            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(key_url).spawn();
            #[cfg(target_os = "linux")]
            let _ = std::process::Command::new("xdg-open")
                .arg(key_url)
                .spawn();
            #[cfg(target_os = "windows")]
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", key_url])
                .spawn();

            println!("Create a new API key, then paste it here.\n");

            let api_key = Password::new(&format!("{}:", env_var))
                .without_confirmation()
                .prompt()
                .map_err(|e| CliError::General(e.to_string()))?;

            if api_key.is_empty() {
                println!("No key provided, aborting.");
                return Ok(());
            }

            save_api_key(env_var, &api_key)?;
            unsafe {
                std::env::set_var(env_var, &api_key);
            }
            println!("Saved to ~/.config/abbot/keys.env\n");

            print!("Fetching models... ");
            match refresh_provider(&provider).await {
                Ok(cache) => println!("{} models cached", cache.models.len()),
                Err(e) => println!("error - {}", e),
            }

            print!("Testing connection... ");
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|e| CliError::General(e.to_string()))?;

            let status = match provider.as_str() {
                "anthropic" => {
                    test_anthropic(&client, "https://api.anthropic.com/v1", Some(&api_key)).await
                }
                "openai" => {
                    test_openai(&client, "https://api.openai.com/v1", Some(&api_key)).await
                }
                "openrouter" => {
                    test_openrouter(
                        &client,
                        "https://openrouter.ai/api/v1",
                        Some(&api_key),
                    )
                    .await
                }
                _ => "ok".to_string(),
            };

            if status == "ok" {
                println!("ok\n");
                println!("To use {} models:", provider);
                println!("  abbot providers use {}", default_model);
            } else {
                println!("failed - {}\n", status);
                println!("Check your API key and try again.");
            }
        }

        ProvidersAction::Add { provider } => {
            use inquire::{Confirm, Password, Select};

            let provider = provider.to_lowercase();

            let (env_var, default_model, needs_key) = match provider.as_str() {
                "anthropic" => (
                    "ANTHROPIC_API_KEY",
                    "anthropic/claude-sonnet-4-20250514",
                    true,
                ),
                "openai" => ("OPENAI_API_KEY", "openai/gpt-4.1", true),
                "openrouter" => (
                    "OPENROUTER_API_KEY",
                    "openrouter/anthropic/claude-sonnet-4",
                    true,
                ),
                "ollama" => ("", "ollama/llama3.2", false),
                _ => {
                    println!("Unknown provider: {}", provider);
                    println!("Available: anthropic, openai, openrouter, ollama");
                    return Ok(());
                }
            };

            if needs_key {
                let existing = std::env::var(env_var).ok();

                if existing.is_some() {
                    println!("{} is already configured.", env_var);
                    let replace = Confirm::new("Replace existing key?")
                        .with_default(false)
                        .prompt()
                        .map_err(|e| CliError::General(e.to_string()))?;

                    if !replace {
                        println!("Keeping existing key.");
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
                            println!("Saved to ~/.config/abbot/keys.env");
                        }
                    }
                } else {
                    let api_key = Password::new(&format!("{}:", env_var))
                        .without_confirmation()
                        .prompt()
                        .map_err(|e| CliError::General(e.to_string()))?;

                    if api_key.is_empty() {
                        println!("No key provided, skipping.");
                        return Ok(());
                    }

                    save_api_key(env_var, &api_key)?;
                    unsafe {
                        std::env::set_var(env_var, &api_key);
                    }
                    println!("Saved to ~/.config/abbot/keys.env");
                }
            } else {
                println!("Ollama doesn't require an API key.");
            }

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

            let models: Vec<ModelOption> = match refresh_provider(&provider).await {
                Ok(cache) => {
                    println!("{} models cached", cache.models.len());
                    cache
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
                                display: format!(
                                    "{:<45} {:>12}  ctx:{}",
                                    m.id, price_info, ctx
                                ),
                            }
                        })
                        .collect()
                }
                Err(e) => {
                    println!("failed ({})", e);
                    vec![ModelOption {
                        id: default_model.to_string(),
                        display: default_model.to_string(),
                    }]
                }
            };

            let set_default = Confirm::new("Set as default model?")
                .with_default(true)
                .prompt()
                .map_err(|e| CliError::General(e.to_string()))?;

            if set_default {
                let model = if models.len() > 1 {
                    Select::new("Select model:", models)
                        .prompt()
                        .map_err(|e| CliError::General(e.to_string()))?
                        .id
                } else {
                    default_model.to_string()
                };

                let full_model = if model.starts_with(&format!("{}/", provider))
                    || provider == "openrouter"
                {
                    model
                } else {
                    format!("{}/{}", provider, model)
                };

                update_config_model(&full_model)?;
                println!("Updated config to use: {}", full_model);
            }

            println!("\nProvider configured. Restart abbot to use the new settings.");
        }

        ProvidersAction::Remove { provider } => {
            let provider = provider.to_lowercase();

            let env_var = match provider.as_str() {
                "anthropic" => "ANTHROPIC_API_KEY",
                "openai" => "OPENAI_API_KEY",
                "openrouter" => "OPENROUTER_API_KEY",
                "ollama" => {
                    println!("Ollama has no API key to remove.");
                    return Ok(());
                }
                _ => {
                    println!("Unknown provider: {}", provider);
                    return Ok(());
                }
            };

            remove_api_key(env_var)?;
            println!("Removed {} from keys.env", env_var);
        }

        ProvidersAction::Test => {
            use abbot::runtime::AppConfig;

            println!("Testing provider configurations...\n");

            config::init_app_config(None);

            let app_config = AppConfig::global();
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|e| CliError::General(e.to_string()))?;

            let providers = [
                ("openrouter", "OPENROUTER_API_KEY", true),
                ("anthropic", "ANTHROPIC_API_KEY", true),
                ("openai", "OPENAI_API_KEY", true),
                ("ollama", "", false),
            ];

            let mut results: Vec<(&str, &str, String)> = Vec::new();

            for (name, env_var, needs_key) in providers {
                let provider_config = app_config.providers.get(name);
                let base_url = provider_config
                    .and_then(|p| p.base_url.as_deref())
                    .unwrap_or(match name {
                        "openrouter" => "https://openrouter.ai/api/v1",
                        "anthropic" => "https://api.anthropic.com/v1",
                        "openai" => "https://api.openai.com/v1",
                        "ollama" => "http://localhost:11434/v1",
                        _ => "",
                    });

                let api_key = if needs_key {
                    std::env::var(env_var).ok()
                } else {
                    None
                };

                let status = match name {
                    "openrouter" => {
                        test_openrouter(&client, base_url, api_key.as_deref()).await
                    }
                    "anthropic" => {
                        test_anthropic(&client, base_url, api_key.as_deref()).await
                    }
                    "openai" => test_openai(&client, base_url, api_key.as_deref()).await,
                    "ollama" => test_ollama(&client, base_url).await,
                    _ => "unknown provider".to_string(),
                };

                results.push((name, base_url, status));
            }

            for (name, base_url, status) in &results {
                let icon = if status == "ok" { "ok" } else { "FAIL" };
                println!("  {:<12} [{:>4}] {}", name, icon, base_url);
                if *status != "ok" {
                    println!("               {}", status);
                }
            }

            let ok_count = results.iter().filter(|(_, _, s)| s == "ok").count();
            println!("\n{}/{} providers working", ok_count, results.len());
        }

        ProvidersAction::Use { model } => {
            let model = model.trim();

            if !model.contains('/') {
                println!("Error: model must be in provider/model format (e.g., anthropic/claude-3-5-haiku-latest)");
                return Ok(());
            }

            let provider = model.split('/').next().unwrap_or("");
            let known_providers = ["anthropic", "openai", "openrouter", "ollama"];
            if !known_providers.contains(&provider) {
                println!(
                    "Warning: unknown provider '{}'. Known providers: {:?}",
                    provider, known_providers
                );
            }

            update_config_model(model)?;
            println!("Updated all model configs to: {}", model);
            println!("\nRestart abbot for changes to take effect.");
        }
    }

    Ok(())
}

// =============================================================================
// PROVIDER FETCH / TEST HELPERS
// =============================================================================

fn format_price(cost: Option<f64>) -> String {
    match cost {
        None => "-".to_string(),
        Some(c) if c == 0.0 => "free".to_string(),
        Some(c) => format!("${:.2}", c * 1_000_000.0),
    }
}

fn query_ollama_models() -> Vec<String> {
    let output = match std::process::Command::new("ollama").arg("list").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .skip(1)
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

async fn fetch_openrouter_models() -> Result<Vec<CachedModel>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("OpenRouter API error: {}", resp.status()).into());
    }

    let json: serde_json::Value = resp.json().await?;
    let models = json["data"]
        .as_array()
        .ok_or("invalid response format")?
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let name = m["name"].as_str().map(|s| s.to_string());
            let context_window = m["context_length"].as_u64();
            let input_cost = m["pricing"]["prompt"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok());
            let output_cost = m["pricing"]["completion"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok());
            Some(CachedModel {
                id,
                name,
                context_window,
                input_cost,
                output_cost,
            })
        })
        .collect();

    Ok(models)
}

fn anthropic_model_info(id: &str) -> (Option<u64>, Option<f64>, Option<f64>) {
    match id {
        "claude-sonnet-4-20250514" | "claude-sonnet-4-latest" => {
            (
                Some(200_000),
                Some(3.0 / 1_000_000.0),
                Some(15.0 / 1_000_000.0),
            )
        }
        "claude-3-5-sonnet-20241022" | "claude-3-5-sonnet-latest" | "claude-3-5-sonnet-20240620" => {
            (
                Some(200_000),
                Some(3.0 / 1_000_000.0),
                Some(15.0 / 1_000_000.0),
            )
        }
        "claude-3-5-haiku-20241022" | "claude-3-5-haiku-latest" => {
            (
                Some(200_000),
                Some(0.80 / 1_000_000.0),
                Some(4.0 / 1_000_000.0),
            )
        }
        "claude-3-opus-20240229" | "claude-3-opus-latest" => {
            (
                Some(200_000),
                Some(15.0 / 1_000_000.0),
                Some(75.0 / 1_000_000.0),
            )
        }
        "claude-3-sonnet-20240229" => {
            (
                Some(200_000),
                Some(3.0 / 1_000_000.0),
                Some(15.0 / 1_000_000.0),
            )
        }
        "claude-3-haiku-20240307" => {
            (
                Some(200_000),
                Some(0.25 / 1_000_000.0),
                Some(1.25 / 1_000_000.0),
            )
        }
        _ => (None, None, None),
    }
}

async fn fetch_anthropic_models() -> Result<Vec<CachedModel>, Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| "ANTHROPIC_API_KEY not set")?;

    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("Anthropic API error: {}", resp.status()).into());
    }

    let json: serde_json::Value = resp.json().await?;
    let models = json["data"]
        .as_array()
        .ok_or("invalid response format")?
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let name = m["display_name"].as_str().map(|s| s.to_string());
            let (context_window, input_cost, output_cost) = anthropic_model_info(&id);
            Some(CachedModel {
                id,
                name,
                context_window,
                input_cost,
                output_cost,
            })
        })
        .collect();

    Ok(models)
}

async fn fetch_openai_models() -> Result<Vec<CachedModel>, Box<dyn std::error::Error>> {
    let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| "OPENAI_API_KEY not set")?;

    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.openai.com/v1/models")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("OpenAI API error: {}", resp.status()).into());
    }

    let json: serde_json::Value = resp.json().await?;
    let models = json["data"]
        .as_array()
        .ok_or("invalid response format")?
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            Some(CachedModel {
                id,
                name: None,
                context_window: None,
                input_cost: None,
                output_cost: None,
            })
        })
        .collect();

    Ok(models)
}

fn fetch_ollama_models_cached() -> Vec<CachedModel> {
    query_ollama_models()
        .into_iter()
        .map(|id| CachedModel {
            id,
            name: None,
            context_window: None,
            input_cost: None,
            output_cost: None,
        })
        .collect()
}

async fn refresh_provider(provider: &str) -> Result<ProviderCache, Box<dyn std::error::Error>> {
    let models = match provider {
        "openrouter" => fetch_openrouter_models().await?,
        "anthropic" => fetch_anthropic_models().await?,
        "openai" => fetch_openai_models().await?,
        "ollama" => fetch_ollama_models_cached(),
        _ => return Err(format!("unknown provider: {}", provider).into()),
    };

    let cache = ProviderCache {
        provider: provider.to_string(),
        fetched_at: chrono::Utc::now().to_rfc3339(),
        models,
    };

    save_provider_cache(&cache)?;
    Ok(cache)
}

// Public test functions used by info command
pub async fn test_openrouter(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> String {
    let url = format!("{}/models", base_url.trim_end_matches('/'));

    let mut req = client.get(&url);
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {}", key));
    }

    match req.send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                "ok".to_string()
            } else {
                format!("HTTP {}", resp.status())
            }
        }
        Err(e) => {
            if e.is_timeout() {
                "timeout".to_string()
            } else if e.is_connect() {
                "connection failed".to_string()
            } else {
                format!("{}", e)
            }
        }
    }
}

pub async fn test_anthropic(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> String {
    let Some(key) = api_key else {
        return "ANTHROPIC_API_KEY not set".to_string();
    };

    let url = format!("{}/models", base_url.trim_end_matches('/'));

    match client
        .get(&url)
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                "ok".to_string()
            } else if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                "invalid API key".to_string()
            } else {
                format!("HTTP {}", resp.status())
            }
        }
        Err(e) => {
            if e.is_timeout() {
                "timeout".to_string()
            } else if e.is_connect() {
                "connection failed".to_string()
            } else {
                format!("{}", e)
            }
        }
    }
}

pub async fn test_openai(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> String {
    let Some(key) = api_key else {
        return "OPENAI_API_KEY not set".to_string();
    };

    let url = format!("{}/models", base_url.trim_end_matches('/'));

    match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", key))
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                "ok".to_string()
            } else if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                "invalid API key".to_string()
            } else {
                format!("HTTP {}", resp.status())
            }
        }
        Err(e) => {
            if e.is_timeout() {
                "timeout".to_string()
            } else if e.is_connect() {
                "connection failed".to_string()
            } else {
                format!("{}", e)
            }
        }
    }
}

pub async fn test_ollama(client: &reqwest::Client, base_url: &str) -> String {
    let base = base_url.trim_end_matches("/v1").trim_end_matches('/');
    let url = format!("{}/api/tags", base);

    match client.get(&url).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                "ok".to_string()
            } else {
                format!("HTTP {}", resp.status())
            }
        }
        Err(e) => {
            if e.is_timeout() {
                "timeout".to_string()
            } else if e.is_connect() {
                "not running (connection refused)".to_string()
            } else {
                format!("{}", e)
            }
        }
    }
}
