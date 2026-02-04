// Abbot - Persistent AI background daemon.
//
// Runs a heartbeat loop scoped to the starting directory.
// The kernel is the nervous system for a single collective:
// - 1 Head (decision maker, will scale to multiple later)
// - 1 Mind (reflection, long-term memory)
// - N Hands (task executors)
//
// Timing model:
// - Tick: 60 seconds (fixed)
// - Default sleep: 300 seconds (5 ticks)
// - Wake debounce: 5 seconds

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use abbot::Scope;
use abbot::history::Store;
use abbot::recall::{Indexer, Ollama, Search, ensure_schema as ensure_recall_schema};
use abbot::runtime::{
    AppConfig, HandService, HeadConfig, HeadService, Kernel, MindService, ProcService,
    SessionWriteLocks,
};
use abbot::server::Server;

const DEFAULT_HEAD_ID: &str = "Abbot";
// Legacy bus-based harness tick constants removed.

#[derive(Parser, Clone)]
#[command(name = "abbot")]
#[command(about = "Abbot: persistent AI background daemon", version)]
struct Cli {
    /// Path to config file (default: ~/.config/abbot/abbot.toml)
    #[arg(long)]
    config: Option<PathBuf>,

    /// API server address (host:port)
    #[arg(long)]
    addr: Option<String>,

    /// Exit after processing (use with `run prompt` for testing)
    #[arg(long)]
    exit: bool,

    /// Proxy mode: forward OpenAI-compatible requests to an upstream backend unchanged
    #[arg(long)]
    proxy: bool,

    /// Convene a conclave on boot (first-boot init or regular boot)
    #[arg(long)]
    conclave: bool,

    /// Log output format: default, compact, pretty
    #[arg(long)]
    log_format: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Clone)]
enum Command {
    /// Run the daemon (default), optionally with a frontend
    Run {
        #[command(subcommand)]
        frontend: Option<RunFrontend>,
    },
    /// Reset workspace state (databases, memory, config)
    Reset {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
        /// Also delete and regenerate config file
        #[arg(long)]
        config: bool,
    },
    /// Memory index management
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    /// OpenCode integration
    Opencode {
        #[command(subcommand)]
        action: OpencodeAction,
    },
    /// Manage workspace plugins (tools)
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Manage model providers
    Providers {
        #[command(subcommand)]
        action: ProvidersAction,
    },
}

#[derive(clap::Subcommand, Clone)]
enum PluginAction {
    /// Detect installed plugins and show versions
    Detect,
    /// Set plugin access level (none, read, write)
    Set {
        /// Plugin name
        name: String,
        /// Access level: none, read, write
        level: String,
    },
    /// List available plugins and their status
    List,
}

#[derive(clap::Subcommand, Clone)]
enum ProvidersAction {
    /// Refresh model lists from all providers
    Refresh,
    /// List cached providers and model counts
    List,
    /// Add/configure a provider with API key
    Add {
        /// Provider name: anthropic, openai, openrouter, ollama
        provider: String,
    },
    /// Remove a provider's API key
    Remove {
        /// Provider name to remove
        provider: String,
    },
}

#[derive(clap::Subcommand, Clone)]
enum RunFrontend {
    /// Run with opencode TUI frontend
    Opencode {
        /// Additional arguments to pass to opencode
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Run the daemon and enqueue a prompt as a need
    Prompt {
        /// Prompt to enqueue
        prompt: String,
    },
    /// Run with claude CLI frontend
    Claude {
        /// Additional arguments to pass to claude
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Run with web UI (opens browser)
    Web,
}

#[derive(clap::Subcommand, Clone)]
enum MemoryAction {
    /// Index transcript files from a directory
    Index {
        /// Directory containing transcript files
        path: PathBuf,
    },
    /// Show memory index statistics
    Stats,
    /// Search memory for a query
    Search {
        /// Search query
        query: Vec<String>,
    },
    /// Wipe all memory data
    Wipe,
}

#[derive(clap::Subcommand, Clone)]
enum OpencodeAction {
    /// Register abbot as an OpenCode provider
    Register,
}


// Legacy harness state removed.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command.clone() {
        None | Some(Command::Run { frontend: None }) => run_daemon(cli, None, None).await,
        Some(Command::Run {
            frontend: Some(RunFrontend::Prompt { prompt }),
        }) => run_daemon(cli, None, Some(prompt)).await,
        Some(Command::Run { frontend: Some(f) }) => run_daemon(cli, Some(f), None).await,
        Some(Command::Reset { force, config }) => run_reset(cli.clone(), force, config),
        Some(Command::Memory { action }) => run_memory(cli.clone(), action.clone()).await,
        Some(Command::Opencode { action }) => run_opencode(cli.clone(), action.clone()).await,
        Some(Command::Plugin { action }) => run_plugin(cli.clone(), action.clone()),
        Some(Command::Providers { action }) => run_providers(action.clone()).await,
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
        .skip(1) // skip header row
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

fn providers_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("abbot").join("providers"))
}

fn keys_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("abbot").join("keys.env"))
}

/// Load API keys from ~/.config/abbot/keys.env and set as environment variables.
/// Returns the keys that were loaded (for display purposes).
fn load_api_keys() -> Vec<(String, String)> {
    let path = match keys_path() {
        Some(p) => p,
        None => return Vec::new(),
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut loaded = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty() && !value.is_empty() {
                // Safety: we're setting env vars at startup before spawning threads
                unsafe {
                    std::env::set_var(key, value);
                }
                // Mask the value for display
                let masked = if value.len() > 8 {
                    format!("{}...{}", &value[..4], &value[value.len() - 4..])
                } else {
                    "****".to_string()
                };
                loaded.push((key.to_string(), masked));
            }
        }
    }
    loaded
}

/// Save an API key to ~/.config/abbot/keys.env
fn save_api_key(key_name: &str, key_value: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = keys_path().ok_or("could not determine keys path")?;

    // Read existing content
    let mut lines: Vec<String> = if path.exists() {
        std::fs::read_to_string(&path)?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        vec![
            "# Abbot API Keys".to_string(),
            "# This file is loaded by the daemon on startup".to_string(),
            "".to_string(),
        ]
    };

    // Update or append the key
    let key_line = format!("{}={}", key_name, key_value);
    let mut found = false;
    for line in &mut lines {
        if line.starts_with(&format!("{}=", key_name)) {
            *line = key_line.clone();
            found = true;
            break;
        }
    }
    if !found {
        lines.push(key_line);
    }

    // Write with 0600 permissions
    let content = lines.join("\n") + "\n";

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&path, &content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

/// Remove an API key from ~/.config/abbot/keys.env
fn remove_api_key(key_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = keys_path().ok_or("could not determine keys path")?;

    if !path.exists() {
        return Ok(());
    }

    let lines: Vec<String> = std::fs::read_to_string(&path)?
        .lines()
        .filter(|line| !line.starts_with(&format!("{}=", key_name)))
        .map(|s| s.to_string())
        .collect();

    let content = lines.join("\n") + "\n";
    std::fs::write(&path, &content)?;
    Ok(())
}

/// Update the model in ~/.config/abbot/abbot.toml for head, hand, and mind.
fn update_config_model(model: &str) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::default_config_path;

    let config_path = default_config_path().ok_or("could not determine config path")?;

    if !config_path.exists() {
        return Err("config file not found, run 'abbot run' first to generate it".into());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut new_lines = Vec::new();

    for line in content.lines() {
        if line.trim().starts_with("model = ") {
            new_lines.push(format!("model = \"{}\"", model));
        } else {
            new_lines.push(line.to_string());
        }
    }

    std::fs::write(&config_path, new_lines.join("\n") + "\n")?;
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct ProviderCache {
    provider: String,
    fetched_at: String,
    models: Vec<CachedModel>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct CachedModel {
    id: String,
    name: Option<String>,
    context_window: Option<u64>,
    /// Cost per input token in USD (e.g., 0.000003 = $3 per 1M tokens)
    #[serde(default)]
    input_cost: Option<f64>,
    /// Cost per output token in USD
    #[serde(default)]
    output_cost: Option<f64>,
}

fn load_provider_cache(provider: &str) -> Option<ProviderCache> {
    let path = providers_dir()?.join(format!("{}.json", provider));
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_provider_cache(cache: &ProviderCache) -> Result<(), Box<dyn std::error::Error>> {
    let dir = providers_dir().ok_or("could not determine providers directory")?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", cache.provider));
    let content = serde_json::to_string_pretty(cache)?;
    std::fs::write(&path, content)?;
    Ok(())
}

/// Default workspace path: ~/.local/abbot
fn default_workspace_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".local").join("abbot"))
}

/// Generate default config for zero-step startup.
/// Returns true if config was created, false if it already existed.
fn ensure_default_config() -> Result<bool, Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{config_dir, default_config_path};

    let config_path = default_config_path().ok_or("could not determine config path")?;

    if config_path.exists() {
        return Ok(false);
    }

    let config_dir = config_dir().ok_or("could not determine config directory")?;
    std::fs::create_dir_all(&config_dir)?;

    let workspace = default_workspace_path()
        .ok_or("could not determine workspace path")?
        .to_string_lossy()
        .to_string();

    let config_content = format!(
        r#"# Abbot configuration
# Auto-generated for zero-step startup
# Run 'abbot init' to reconfigure interactively

workspace = "{workspace}"

[server]
addr = "127.0.0.1:8080"
log_format = "default"
reset_on_single_user_message = true

[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"

[providers.anthropic]
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"

[providers.openai]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[providers.ollama]
base_url = "http://localhost:11434/v1"
api_key_env = ""

[head]
model = "openrouter/openrouter/free"
temperature = 0.7
heartbeat_tick = 30
debounce_ms = 500

[hand]
model = "openrouter/openrouter/free"
temperature = 0.3
max_iters = 24

[mind]
model = "openrouter/openrouter/free"
tick_interval = 60

[pool]
size = 4
timeout_secs = 300
"#,
        workspace = workspace,
    );

    std::fs::write(&config_path, &config_content)?;
    Ok(true)
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
            // Pricing is in string format like "0.000003"
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
            // Anthropic API doesn't include pricing, we could hardcode known values
            Some(CachedModel {
                id,
                name,
                context_window: None,
                input_cost: None,
                output_cost: None,
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
            // OpenAI API doesn't include pricing
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

async fn run_providers(action: ProvidersAction) -> Result<(), Box<dyn std::error::Error>> {
    // Load API keys from ~/.config/abbot/keys.env
    load_api_keys();

    match action {
        ProvidersAction::Refresh => {
            let dir = providers_dir().ok_or("could not determine providers directory")?;
            std::fs::create_dir_all(&dir)?;
            println!("Refreshing provider model lists...\n");

            // OpenRouter (always available, no auth)
            print!("openrouter: ");
            match refresh_provider("openrouter").await {
                Ok(cache) => println!("{} models", cache.models.len()),
                Err(e) => println!("error - {}", e),
            }

            // Anthropic (requires API key)
            print!("anthropic:  ");
            if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                match refresh_provider("anthropic").await {
                    Ok(cache) => println!("{} models", cache.models.len()),
                    Err(e) => println!("error - {}", e),
                }
            } else {
                println!("skipped (ANTHROPIC_API_KEY not set)");
            }

            // OpenAI (requires API key)
            print!("openai:     ");
            if std::env::var("OPENAI_API_KEY").is_ok() {
                match refresh_provider("openai").await {
                    Ok(cache) => println!("{} models", cache.models.len()),
                    Err(e) => println!("error - {}", e),
                }
            } else {
                println!("skipped (OPENAI_API_KEY not set)");
            }

            // Ollama (local)
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
            let dir = providers_dir().ok_or("could not determine providers directory")?;

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

        ProvidersAction::Add { provider } => {
            use inquire::{Confirm, Password, Select};

            let provider = provider.to_lowercase();

            let (env_var, default_model, needs_key) = match provider.as_str() {
                "anthropic" => ("ANTHROPIC_API_KEY", "anthropic/claude-sonnet-4-20250514", true),
                "openai" => ("OPENAI_API_KEY", "openai/gpt-4.1", true),
                "openrouter" => ("OPENROUTER_API_KEY", "openrouter/anthropic/claude-sonnet-4", true),
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
                        .prompt()?;

                    if !replace {
                        println!("Keeping existing key.");
                    } else {
                        let api_key = Password::new(&format!("{}:", env_var))
                            .without_confirmation()
                            .prompt()?;

                        if !api_key.is_empty() {
                            save_api_key(env_var, &api_key)?;
                            // Set in current process for refresh_provider
                            unsafe { std::env::set_var(env_var, &api_key); }
                            println!("Saved to ~/.config/abbot/keys.env");
                        }
                    }
                } else {
                    let api_key = Password::new(&format!("{}:", env_var))
                        .without_confirmation()
                        .prompt()?;

                    if api_key.is_empty() {
                        println!("No key provided, skipping.");
                        return Ok(());
                    }

                    save_api_key(env_var, &api_key)?;
                    // Set in current process for refresh_provider
                    unsafe { std::env::set_var(env_var, &api_key); }
                    println!("Saved to ~/.config/abbot/keys.env");
                }
            } else {
                println!("Ollama doesn't require an API key.");
            }

            // Fetch and cache the provider's models list
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

            fn format_price(cost: Option<f64>) -> String {
                match cost {
                    None => "-".to_string(),
                    Some(c) if c == 0.0 => "free".to_string(),
                    Some(c) => format!("${:.2}", c * 1_000_000.0), // per 1M tokens
                }
            }

            let models: Vec<ModelOption> = match refresh_provider(&provider).await {
                Ok(cache) => {
                    println!("{} models cached", cache.models.len());
                    cache.models.iter().take(20).map(|m| {
                        let price_info = format!(
                            "{} / {}",
                            format_price(m.input_cost),
                            format_price(m.output_cost)
                        );
                        let ctx = m.context_window
                            .map(|c| format!("{}k", c / 1000))
                            .unwrap_or_else(|| "-".to_string());
                        ModelOption {
                            id: m.id.clone(),
                            display: format!("{:<45} {:>12}  ctx:{}", m.id, price_info, ctx),
                        }
                    }).collect()
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
                .prompt()?;

            if set_default {
                let model = if models.len() > 1 {
                    Select::new("Select model:", models)
                        .prompt()?
                        .id
                } else {
                    default_model.to_string()
                };

                let full_model = if model.starts_with(&format!("{}/", provider)) || provider == "openrouter" {
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
    }

    Ok(())
}


fn run_reset(cli: Cli, force: bool, reset_config: bool) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{WorkspacePaths, config_dir, default_config_path};
    use inquire::Confirm;

    // Load config to get workspace path
    if let Some(ref path) = cli.config {
        AppConfig::init(path);
    } else if let Some(path) = default_config_path() {
        if path.exists() {
            AppConfig::init(&path);
        }
    }

    let workspace = AppConfig::global().workspace_path().ok();
    let config_path = cli.config.clone().or_else(default_config_path);

    // Show what will be deleted
    println!("This will delete:\n");

    if let Some(ref ws) = workspace {
        if ws.exists() {
            let paths = WorkspacePaths::new(ws.clone());
            for (name, path) in [
                ("store.db (conversation history)", &paths.store_db),
                ("recall.db (memory embeddings)", &paths.recall_db),
                ("ems.db (entity storage)", &paths.ems_db),
                ("logs.db (frame logs)", &paths.logs_db),
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
            .prompt()?;

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
                ("logs.db", &paths.logs_db),
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
        println!("Run 'abbot run' to regenerate config with defaults.");
    }

    Ok(())
}

async fn run_daemon(
    cli: Cli,
    frontend: Option<RunFrontend>,
    initial_prompt: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::ems::EmsService;
    use abbot::runtime::app_config::{WorkspacePaths, default_config_path};

    // Auto-create default config if none exists (zero-step startup)
    let first_run = if cli.config.is_none() {
        ensure_default_config()?
    } else {
        false
    };

    // Load API keys from ~/.config/abbot/keys.env
    let loaded_keys = load_api_keys();
    if !loaded_keys.is_empty() {
        for (key, masked) in &loaded_keys {
            eprintln!("  loaded {} = {}", key, masked);
        }
    }

    // Initialize config first (before logging setup so we can get workspace path for logs)
    if let Some(ref path) = cli.config {
        AppConfig::init(path);
    } else if let Some(path) = default_config_path() {
        AppConfig::init(&path);
    } else {
        AppConfig::init_default();
    }

    // Get workspace paths from config
    let workspace = AppConfig::global()
        .workspace_path()
        .map_err(|e| format!("workspace configuration error: {}", e))?;

    // Auto-create workspace directory if it doesn't exist
    if !workspace.exists() {
        std::fs::create_dir_all(&workspace)?;
        eprintln!("Created workspace: {}", workspace.display());
    }

    // Print first-run message after workspace is ready
    if first_run {
        eprintln!();
        eprintln!("Welcome to Abbot!");
        eprintln!();
        eprintln!("  Config:    ~/.config/abbot/abbot.toml");
        eprintln!("  Workspace: {}", workspace.display());
        eprintln!("  Model:     openrouter/free (no API key required)");
        eprintln!();
        eprintln!("To use Claude or GPT, run: abbot providers add <provider>");
        eprintln!();
    }

    let paths = WorkspacePaths::new(workspace.clone());

    // Resolve server bind addr + logging format now that config is loaded.
    let bind_addr = cli
        .addr
        .clone()
        .or_else(|| AppConfig::global().server.addr.clone())
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    abbot::runtime::set_effective_bind_addr(bind_addr.clone());

    let log_format = cli
        .log_format
        .clone()
        .or_else(|| AppConfig::global().server.log_format.clone())
        .unwrap_or_else(|| "default".to_string());

    // When running with a TUI frontend, redirect logs to a file to avoid corrupting the display
    let is_tui = matches!(
        frontend,
        Some(RunFrontend::Opencode { .. }) | Some(RunFrontend::Claude { .. })
    );
    if is_tui {
        let log_path = workspace.join("daemon.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        init_logging(&log_format, Some(log_file), false);
    } else {
        init_logging(&log_format, None, true);
    }

    tracing::debug!(config = ?AppConfig::global(), "app config loaded");

    // Create <workspace>/root/ if missing
    if !paths.root.exists() {
        std::fs::create_dir_all(&paths.root)?;
        tracing::info!(path = %paths.root.display(), "created root directory");
    }

    // Create <workspace>/mind/ if missing
    if !paths.mind.exists() {
        std::fs::create_dir_all(&paths.mind)?;
        tracing::info!(path = %paths.mind.display(), "created mind directory");
    }

    // Resolve database paths.
    let db_path = paths.store_db.clone();
    let recall_db_path = paths.recall_db.clone();
    let ems_db_path = paths.ems_db.clone();
    let logs_db_path = paths.logs_db.clone();

    tracing::info!(
        workspace = %workspace.display(),
        "abbot starting"
    );

    if cli.proxy {
        tracing::info!(addr = %bind_addr, "starting in proxy mode");

        if initial_prompt.is_some() || cli.exit {
            tracing::warn!("prompt/--exit are ignored in --proxy mode");
        }
        if frontend.is_some() {
            tracing::warn!("frontend launch is ignored in --proxy mode");
        }

        let store = Arc::new(Store::open(":memory:")?);
        Server::new(store, DEFAULT_HEAD_ID)
            .with_addr(&bind_addr)
            .with_proxy(true)
            .spawn();

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("shutdown requested");
                    break;
                }
                _ = std::future::pending::<()>() => {}
            }
        }

        return Ok(());
    }

    // Set working directory to workspace/root (the VFS root)
    std::env::set_current_dir(&paths.root)?;

    // Initialize kernel syscall dispatcher with VFS auto-mount
    Kernel::init(&workspace);

    let store = Arc::new(Store::open(&db_path)?);
    tracing::debug!(db = %db_path.display(), "database opened");

    if let Some(k) = Kernel::get() {
        k.set_store(store.clone());
    }

    // Kernel frame audit log.
    match abbot::kernel::AuditLog::open(&logs_db_path) {
        Ok(audit) => {
            if let Some(k) = Kernel::get() {
                k.set_audit(audit).await;
            }
            tracing::debug!(db = %logs_db_path.display(), "logs database opened");
        }
        Err(e) => {
            tracing::warn!(error = %e, db = %logs_db_path.display(), "failed to open logs database");
        }
    }

    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let memory_search: Option<Arc<Search>> = match rusqlite::Connection::open(&recall_db_path) {
        Ok(conn) => {
            if let Err(e) = ensure_recall_schema(&conn) {
                tracing::warn!(error = %e, "failed to init memory schema");
                None
            } else {
                tracing::debug!(db = %recall_db_path.display(), "recall database opened");
                Some(Arc::new(Search::new(conn, Ollama::local())))
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to open recall database");
            None
        }
    };

    let ems_handle = match EmsService::open(&ems_db_path) {
        Ok(svc) => {
            tracing::debug!(db = %ems_db_path.display(), "EMS database opened");
            Some(svc.handle())
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to open EMS database");
            None
        }
    };

    let proc = ProcService::new().handle();

    let snapshot = abbot::runtime::SnapshotManager::new(paths.root.clone(), Some(store.clone()));

    // NeedService is replaced by kernel-managed need syscalls (need:enqueue/lease/fulfill).
    // StatService / RecallFlushService intentionally disabled for now.
    // Idle monitoring is now handled by MindService via kernel activity + queue state.

    let mut hand = HandService::new(store.clone(), paths.root.clone(), snapshot.clone());
    if let Some(ref ems) = ems_handle {
        hand = hand.with_ems(ems.clone());
    }
    Arc::new(hand).start();

    // Start head pool (kernel need queue dispatches needs to these)
    let head_cfg = HeadConfig::from_config();
    let session_locks = SessionWriteLocks::new();
    tracing::info!(pool_size = head_cfg.pool_size, "starting head pool");
    for i in 0..head_cfg.pool_size {
        let head_id = format!("head-{}", i);
        let mut head = HeadService::new(
            proc.clone(),
            store.clone(),
            paths.root.clone(),
            &head_id,
            vec![Scope::main()],
            memory_search.clone(),
            snapshot.clone(),
            session_locks.clone(),
        );
        if let Some(ref ems) = ems_handle {
            head = head.with_ems(ems.clone());
        }
        Arc::new(head).start();
    }

    let mind = MindService::new(
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![Scope::main()],
        paths.root.clone(),
    )
    .with_conclave_on_boot(cli.conclave);

    Arc::new(mind).start();

    // Determine web dist path (config override, otherwise relative to manifest/exe)
    let web_dist = AppConfig::global()
        .server
        .web_dist
        .as_deref()
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                        .unwrap_or_else(|| PathBuf::from("."))
                });
            manifest_dir.join("web").join("dist")
        });

    Server::new(store.clone(), DEFAULT_HEAD_ID)
        .with_addr(&bind_addr)
        .with_web_dist(web_dist)
        .spawn();

    // Spawn frontend if requested
    let mut frontend_child: Option<tokio::process::Child> = None;
    if let Some(ref fe) = frontend {
        // Wait for server to be ready
        let health_url = format!("http://{}/health", bind_addr);
        for _ in 0..50 {
            if reqwest::get(&health_url).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        match fe {
            RunFrontend::Opencode { args } => {
                // Update opencode config with current address
                if let Err(e) = update_opencode_config(&bind_addr) {
                    tracing::warn!(error = %e, "failed to update opencode config");
                }

                let model_arg = "abbot/abbot/default";
                tracing::info!(model = model_arg, "launching opencode");

                match tokio::process::Command::new("opencode")
                    .arg("-m")
                    .arg(model_arg)
                    .args(args)
                    .spawn()
                {
                    Ok(c) => frontend_child = Some(c),
                    Err(e) => {
                        tracing::error!(error = %e, "failed to spawn opencode");
                        return Err(e.into());
                    }
                }
            }
            RunFrontend::Claude { args } => {
                let base_url = format!("http://{}", bind_addr);
                tracing::info!(base_url = %base_url, "launching claude");

                match tokio::process::Command::new("claude")
                    .env("ANTHROPIC_BASE_URL", &base_url)
                    .env("ANTHROPIC_API_KEY", "abbot")
                    .args(args)
                    .spawn()
                {
                    Ok(c) => frontend_child = Some(c),
                    Err(e) => {
                        tracing::error!(error = %e, "failed to spawn claude");
                        return Err(e.into());
                    }
                }
            }
            RunFrontend::Web => {
                let url = format!("http://{}", bind_addr);
                tracing::info!(url = %url, "opening browser");

                #[cfg(target_os = "macos")]
                let result = std::process::Command::new("open").arg(&url).spawn();
                #[cfg(target_os = "linux")]
                let result = std::process::Command::new("xdg-open").arg(&url).spawn();
                #[cfg(target_os = "windows")]
                let result = std::process::Command::new("cmd")
                    .args(["/C", "start", &url])
                    .spawn();

                if let Err(e) = result {
                    tracing::warn!(error = %e, "failed to open browser");
                }
            }
            RunFrontend::Prompt { .. } => {}
        }
    }

    let exit = cli.exit;

    if let Some(ref prompt) = initial_prompt {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".into());
        };

        let thread_id = uuid::Uuid::new_v4();
        let scope = Scope::main();

        let _ = k.sigcalls().open(scope.as_str(), thread_id).await;

        // Best-effort log + enqueue.
        let dispatcher = k.dispatcher().await;
        let req = abbot::kernel::Frame::req(
            "log:append",
            serde_json::json!({
                "kind": "chat:user",
                "scope": scope.as_str(),
                "data": {"content": prompt, "reply_to": thread_id.to_string()}
            }),
        )
        .with_actor("human/_user");
        let mut rx = dispatcher.dispatch(
            req,
            k.workspace().to_path_buf(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;

        let need_id = uuid::Uuid::new_v4().to_string();
        let req = abbot::kernel::Frame::req(
            "need:enqueue",
            serde_json::json!({
                "need_id": need_id,
                "source": "user",
                "priority": "normal",
                "need": prompt,
                "context": "",
                "scope": scope.as_str(),
                "reply_to": thread_id.to_string(),
                "reconvene": false,
            }),
        )
        .with_actor("human/_user");
        let mut rx = dispatcher.dispatch(
            req,
            k.workspace().to_path_buf(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;

        if exit {
            // Wait for the reply stream to terminate.
            let mut reply_rx = k.sigcalls().open(scope.as_str(), thread_id).await;
            while let Some(frame) = reply_rx.recv().await {
                if matches!(
                    frame.op,
                    abbot::kernel::FrameOp::Done
                        | abbot::kernel::FrameOp::Ok
                        | abbot::kernel::FrameOp::Error
                        | abbot::kernel::FrameOp::Redirect
                ) {
                    break;
                }
            }
            tracing::info!("exiting (--exit mode)");
            return Ok(());
        }
    }

    // Keep the daemon alive.
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutdown requested");
                break;
            }
            status = async {
                match frontend_child.as_mut() {
                    Some(child) => child.wait().await,
                    None => std::future::pending().await,
                }
            } => {
                match status {
                    Ok(s) if s.success() => tracing::info!("frontend exited"),
                    Ok(s) => tracing::info!(code = ?s.code(), "frontend exited"),
                    Err(e) => tracing::warn!(error = %e, "frontend wait failed"),
                }
                break;
            }
        }
    }

    Ok(())
}

fn init_logging(log_format: &str, file: Option<std::fs::File>, ansi: bool) {
    use tracing_subscriber::fmt::format::FmtSpan;

    let format = log_format.trim().to_ascii_lowercase();

    match (file, format.as_str()) {
        (Some(f), "compact") => tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(f))
            .compact()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),
        (None, "compact") => tracing_subscriber::fmt()
            .compact()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),

        (Some(f), "pretty") => tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(f))
            .pretty()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),
        (None, "pretty") => tracing_subscriber::fmt()
            .pretty()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),

        (Some(f), _) => tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(f))
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),
        (None, _) => tracing_subscriber::fmt()
            .with_ansi(ansi)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .init(),
    };
}

async fn run_memory(cli: Cli, action: MemoryAction) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{WorkspacePaths, default_config_path};

    // Initialize config
    if let Some(ref path) = cli.config {
        AppConfig::init(path);
    } else if let Some(path) = default_config_path() {
        AppConfig::init(&path);
    } else {
        AppConfig::init_default();
    }

    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let workspace = AppConfig::global()
        .workspace_path()
        .map_err(|e| format!("workspace configuration error: {}", e))?;
    let paths = WorkspacePaths::new(workspace);
    let recall_db_path = paths.recall_db.clone();
    let conn = rusqlite::Connection::open(&recall_db_path)?;
    ensure_recall_schema(&conn)?;

    match action {
        MemoryAction::Index { path } => {
            let ollama = Ollama::local();
            let indexer = Indexer::new(conn, ollama);

            println!("Indexing {}...", path.display());
            let start = std::time::Instant::now();

            let result = indexer.index_directory(&path).await?;

            println!(
                "Done in {:.1}s: {} indexed, {} skipped, {} errors",
                start.elapsed().as_secs_f32(),
                result.indexed,
                result.skipped,
                result.errors
            );
        }

        MemoryAction::Stats => {
            let ollama = Ollama::local();
            let search = Search::new(conn, ollama);
            let stats = search.stats()?;

            println!("Recall index stats:");
            println!("  Transcripts: {}", stats.transcripts);
            println!("  Chunks:      {}", stats.chunks);
            println!("  Vectors:     {}", stats.vectors);
        }

        MemoryAction::Search { query } => {
            let query_str = query.join(" ");
            if query_str.is_empty() {
                eprintln!("usage: abbot memory search <query>");
                std::process::exit(1);
            }

            let ollama = Ollama::local();
            let search = Search::new(conn, ollama);

            println!("Searching for: {}\n", query_str);
            let results = search.query(&query_str, 5).await?;

            for (i, r) in results.iter().enumerate() {
                println!(
                    "{}. [dist={:.3}] {} ({})",
                    i + 1,
                    r.distance,
                    r.source,
                    r.file_path
                );
                println!("   project: {:?}", r.project_path);
                println!("   ---");
                let preview: String = r.content.chars().take(200).collect();
                println!("   {}", preview.replace('\n', "\n   "));
                println!();
            }
        }

        MemoryAction::Wipe => {
            conn.execute("DELETE FROM chunk_vectors", [])?;
            conn.execute("DELETE FROM chunks", [])?;
            conn.execute("DELETE FROM transcripts", [])?;
            println!("Memory wiped.");
        }
    }

    Ok(())
}

fn update_opencode_config(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let base_url = format!("http://{}/v1", addr);

    let config_dir = dirs::home_dir()
        .ok_or("could not find home directory")?
        .join(".config")
        .join("opencode");

    std::fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("opencode.json");

    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if config.get("provider").is_none() {
        config["provider"] = serde_json::json!({});
    }

    config["provider"]["abbot"] = serde_json::json!({
        "name": "Abbot",
        "npm": "@ai-sdk/openai-compatible",
        "options": {
            "baseURL": base_url,
            "apiKey": "not-required"
        },
        "models": {
            "abbot/default": {
                "name": "Abbot Default",
                "_launch": true
            }
        }
    });

    let content = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, content)?;

    Ok(())
}

async fn run_opencode(_cli: Cli, action: OpencodeAction) -> Result<(), Box<dyn std::error::Error>> {
    const BASE_URL: &str = "http://localhost:8080/v1";

    match action {
        OpencodeAction::Register => {
            let config_dir = dirs::home_dir()
                .ok_or("could not find home directory")?
                .join(".config")
                .join("opencode");

            std::fs::create_dir_all(&config_dir)?;
            let config_path = config_dir.join("opencode.json");

            let mut config: serde_json::Value = if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
            } else {
                serde_json::json!({})
            };

            if config.get("provider").is_none() {
                config["provider"] = serde_json::json!({});
            }

            config["provider"]["abbot"] = serde_json::json!({
                "name": "Abbot",
                "npm": "@ai-sdk/openai-compatible",
                "options": {
                    "baseURL": BASE_URL,
                    "apiKey": "not-required"
                },
                "models": {
                    "abbot/default": {
                        "name": "Abbot Default",
                        "_launch": true
                    }
                }
            });

            let content = serde_json::to_string_pretty(&config)?;
            std::fs::write(&config_path, content)?;

            println!("Registered abbot provider in {}", config_path.display());
            println!("Run with: abbot run opencode");
        }
    }

    Ok(())
}


fn run_plugin(_cli: Cli, action: PluginAction) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::PluginManager;
    use abbot::runtime::app_config::default_config_path;
    use std::collections::HashMap;
    use std::process::Command;

    // Plugin access levels (matches PluginLevel::Read/Write in runtime)
    #[derive(Debug, Clone, PartialEq)]
    enum PluginLevel {
        None,  // Disabled
        Read,  // Read-only (safe for hands and heads)
        Write, // Full access (heads only for writes)
    }

    impl PluginLevel {
        fn from_str(s: &str) -> Option<Self> {
            match s.to_lowercase().as_str() {
                "none" => Some(Self::None),
                "read" => Some(Self::Read),
                "write" => Some(Self::Write),
                _ => None,
            }
        }

        fn as_str(&self) -> &'static str {
            match self {
                Self::None => "none",
                Self::Read => "read",
                Self::Write => "write",
            }
        }
    }

    // Read [plugins] section from abbot.toml
    fn read_plugins_config(config_path: &std::path::Path) -> HashMap<String, PluginLevel> {
        let raw = match std::fs::read_to_string(config_path) {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        };

        let v: toml::Value = match toml::from_str(&raw) {
            Ok(v) => v,
            Err(_) => return HashMap::new(),
        };

        let mut out = HashMap::new();
        if let Some(plugins) = v.get("plugins").and_then(|v| v.as_table()) {
            for (id, value) in plugins {
                if let Some(level_str) = value.as_str() {
                    if let Some(level) = PluginLevel::from_str(level_str) {
                        out.insert(id.clone(), level);
                    }
                }
            }
        }
        out
    }

    // Update [plugins] section in abbot.toml
    fn update_plugin_level(
        config_path: &std::path::Path,
        plugin_id: &str,
        level: &PluginLevel,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let raw = std::fs::read_to_string(config_path).unwrap_or_default();
        let mut doc: toml::Table = toml::from_str(&raw).unwrap_or_default();

        let plugins = doc
            .entry("plugins".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or("plugins is not a table")?;

        plugins.insert(plugin_id.to_string(), toml::Value::String(level.as_str().to_string()));

        let out = toml::to_string_pretty(&doc)?;
        std::fs::write(config_path, out)?;
        Ok(())
    }

    // Detect if a program is installed and get version
    fn detect_program(program: &str) -> (bool, Option<String>) {
        let which = Command::new("which")
            .arg(program)
            .output();

        let installed = which.map(|o| o.status.success()).unwrap_or(false);
        if !installed {
            return (false, None);
        }

        // Try --version first, then -V, then version
        for flag in ["--version", "-V", "version"] {
            if let Ok(output) = Command::new(program).arg(flag).output() {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let text = if stdout.trim().is_empty() { stderr } else { stdout };
                    let version = text.lines().next().unwrap_or("").trim().to_string();
                    if !version.is_empty() {
                        return (true, Some(version));
                    }
                }
            }
        }

        (true, None)
    }

    let config_path = default_config_path().ok_or("could not determine config path")?;
    let plugins_config = read_plugins_config(&config_path);

    // Get workspace for PluginManager
    let workspace = AppConfig::global().workspace_path().ok();
    let workspace_root = workspace.as_ref().map(|w| w.join("root"));

    match action {
        PluginAction::Detect => {
            println!("Detecting installed plugins...\n");

            let mgr = workspace_root
                .as_ref()
                .map(|r| PluginManager::load_for_workspace_root(r))
                .unwrap_or_else(PluginManager::empty);

            let catalog = mgr.catalog();
            if catalog.is_empty() {
                println!("No built-in plugins found.");
                return Ok(());
            }

            let mut results: Vec<(String, String, bool, Option<String>)> = Vec::new();

            for p in &catalog {
                let (installed, version) = detect_program(&p.program);
                results.push((p.id.clone(), p.program.clone(), installed, version));
            }

            // Print results
            for (id, program, installed, version) in &results {
                let status = if *installed { "found" } else { "not found" };
                let ver = version.as_deref().unwrap_or("");
                let current = plugins_config.get(id).map(|l| l.as_str()).unwrap_or("-");
                println!(
                    "  {:<12} {:<10} {:<10} {}",
                    id, status, current, ver
                );
            }

            let found_count = results.iter().filter(|(_, _, i, _)| *i).count();
            println!("\n{}/{} plugins available", found_count, results.len());
            println!("\nSet access level: abbot plugins set <name> <none|read|write>");
        }

        PluginAction::Set { name, level } => {
            let level = PluginLevel::from_str(&level).ok_or_else(|| {
                format!("invalid level '{}', use: none, read, write", level)
            })?;

            update_plugin_level(&config_path, &name, &level)?;
            println!("Set {} = {}", name, level.as_str());
            println!("Restart abbot to apply changes.");
        }

        PluginAction::List => {
            let mgr = workspace_root
                .as_ref()
                .map(|r| PluginManager::load_for_workspace_root(r))
                .unwrap_or_else(PluginManager::empty);

            let catalog = mgr.catalog();

            println!("Plugins:\n");
            if catalog.is_empty() {
                println!("(no built-in plugins available)");
                return Ok(());
            }

            for p in &catalog {
                let level = plugins_config.get(&p.id).map(|l| l.as_str()).unwrap_or("-");
                println!(
                    "  {:<12} {:<10} {}",
                    p.id, level, p.description
                );
            }

            println!("\nLevels: none (disabled), read (safe), write (full)");
            println!("Set: abbot plugins set <name> <none|read|write>");
            println!("Detect: abbot plugins detect");
        }
    }

    Ok(())
}
