// Abbot - Persistent AI background daemon.
//
// Runs a heartbeat loop scoped to the starting directory.
// The message bus is the nervous system for a single collective:
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

use abbot::bus::Scope;
use abbot::history::Store;
use abbot::recall::{Indexer, Ollama, Search, ensure_schema as ensure_recall_schema};
use abbot::runtime::{
    AppConfig, AutistMode, FeverMode, GenerationMode, HandService, HeadConfig, HeadService, Kernel,
    MindService, ProcService, SessionWriteLocks,
};
use abbot::server::Server;

const DEFAULT_HEAD_ID: &str = "Abbot";
// Legacy bus-based harness tick constants removed.

#[derive(Parser, Clone)]
#[command(name = "abbot")]
#[command(about = "Abbot: persistent AI background daemon", version)]
struct Cli {
    /// Path to config file (default: ~/.config/abbot/abbot.toml)
    #[arg(long, env = "ABBOT_CONFIG")]
    config: Option<PathBuf>,

    /// API server address (host:port)
    #[arg(long, env = "ABBOT_ADDR", default_value = "127.0.0.1:8080")]
    addr: String,

    /// Initial prompt to send (triggers immediate wake)
    #[arg(long)]
    prompt: Option<String>,

    /// Exit after head completes processing (use with --prompt for testing)
    #[arg(long)]
    exit: bool,

    /// Fever mode for Mind layer (mild, hot, delirium, meth)
    #[arg(long, env = "ABBOT_FEVER")]
    fever: Option<String>,

    /// Generation mode for Head layer (boomer, genx, millennial, genz, alpha)
    #[arg(long, env = "ABBOT_GENERATION")]
    generation: Option<String>,

    /// Autist mode for Hand layer (adhd, neurotypical, autist, full-retard)
    #[arg(long, env = "ABBOT_AUTIST")]
    autist: Option<String>,

    /// Convene a conclave on boot (first-boot init or regular boot)
    #[arg(long)]
    conclave: bool,

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
    /// Initialize Abbot (create config files)
    Init {
        /// Force reset: overwrite config files and wipe workspace state
        #[arg(long)]
        force: bool,
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
    /// Claude Code integration
    Claude {
        #[command(subcommand)]
        action: ClaudeAction,
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
    /// Enable a plugin for the workspace
    Enable { name: String },
    /// Disable a plugin for the workspace
    Disable { name: String },
    /// List available plugins and their workspace status
    List,
}

#[derive(clap::Subcommand, Clone)]
enum ProvidersAction {
    /// Refresh model lists from all providers
    Refresh,
    /// List cached providers and model counts
    List,
}

#[derive(clap::Subcommand, Clone)]
enum RunFrontend {
    /// Run with opencode TUI frontend
    Opencode {
        /// Additional arguments to pass to opencode
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
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
    /// Run opencode with abbot as the provider
    Run {
        /// Additional arguments to pass to opencode
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(clap::Subcommand, Clone)]
enum ClaudeAction {
    /// Run claude with abbot as the provider
    Run {
        /// Additional arguments to pass to claude
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

// Legacy harness state removed.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command.clone() {
        None | Some(Command::Run { frontend: None }) => run_daemon(cli, None).await,
        Some(Command::Run { frontend: Some(f) }) => run_daemon(cli, Some(f)).await,
        Some(Command::Init { force }) => run_init(cli.clone(), force),
        Some(Command::Memory { action }) => run_memory(cli.clone(), action.clone()).await,
        Some(Command::Opencode { action }) => run_opencode(cli.clone(), action.clone()).await,
        Some(Command::Claude { action }) => run_claude(cli.clone(), action.clone()).await,
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
    }

    Ok(())
}

fn run_init(cli: Cli, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{
        WorkspacePaths, config_dir, default_config_path, default_models_path,
    };
    use inquire::validator::Validation;
    use inquire::{Confirm, Select, Text};
    use std::path::Path;

    // If --force, wipe workspace state first
    if force {
        println!("Resetting Abbot...\n");

        // Try to load existing config to get workspace path
        if let Some(ref path) = cli.config {
            AppConfig::init(path);
        } else if let Some(path) = default_config_path() {
            if path.exists() {
                AppConfig::init(&path);
            }
        }

        if let Ok(workspace) = AppConfig::global().workspace_path() {
            if workspace.exists() {
                println!("Clearing workspace: {}", workspace.display());
                let paths = WorkspacePaths::new(workspace.clone());

                for (name, path) in [
                    ("store.db", &paths.store_db),
                    ("recall.db", &paths.recall_db),
                    ("ems.db", &paths.ems_db),
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

                let head_dir = workspace.join("head");
                if head_dir.exists() {
                    std::fs::remove_dir_all(&head_dir)?;
                    println!("  removed head/");
                }

                let plugins_path = workspace.join("plugins.toml");
                if plugins_path.exists() {
                    std::fs::remove_file(&plugins_path)?;
                    println!("  removed plugins.toml");
                }

                let workspace_config = workspace.join("config.toml");
                if workspace_config.exists() {
                    std::fs::remove_file(&workspace_config)?;
                    println!("  removed config.toml");
                }

                println!();
            }
        }
    }

    println!("Abbot Configuration\n");

    // Create config directory
    let config_dir = config_dir().ok_or("could not determine config directory")?;
    if !config_dir.exists() {
        std::fs::create_dir_all(&config_dir)?;
    }

    // Load existing config for defaults
    let existing_config = default_config_path()
        .filter(|p| p.exists())
        .map(|p| AppConfig::load(&p));

    // Extract existing values
    let existing_workspace = existing_config.as_ref().and_then(|c| c.workspace.clone());
    let existing_provider = existing_config
        .as_ref()
        .and_then(|c| c.model.as_ref())
        .and_then(|m| m.provider.clone());
    let existing_model = existing_config
        .as_ref()
        .and_then(|c| c.model.as_ref())
        .and_then(|m| m.model.clone());
    let existing_pool_size = existing_config.as_ref().and_then(|c| c.pool.size);
    let existing_head_temp = existing_config
        .as_ref()
        .and_then(|c| c.head.llm.temperature);
    let existing_hand_temp = existing_config
        .as_ref()
        .and_then(|c| c.hand.llm.temperature);
    let existing_head_tick = existing_config.as_ref().and_then(|c| c.head.heartbeat_tick);
    let existing_timeout = existing_config.as_ref().and_then(|c| c.pool.timeout_secs);

    // 1. Workspace path
    let default_workspace = existing_workspace.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join("abbot-workspace").to_string_lossy().to_string())
            .unwrap_or_default()
    });

    let workspace_path = Text::new("Workspace path:")
        .with_default(&default_workspace)
        .with_help_message("Absolute path where Abbot stores state and working files")
        .with_validator(|input: &str| {
            let path = Path::new(input);
            if !path.is_absolute() {
                Ok(Validation::Invalid("Path must be absolute".into()))
            } else {
                Ok(Validation::Valid)
            }
        })
        .prompt()?;

    // 2. Provider selection
    #[derive(Clone)]
    struct ProviderOption {
        id: &'static str,
        name: &'static str,
        env_var: &'static str,
        default_model: &'static str,
    }

    impl std::fmt::Display for ProviderOption {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.name)
        }
    }

    let providers = vec![
        ProviderOption {
            id: "anthropic",
            name: "Anthropic (Claude)",
            env_var: "ANTHROPIC_API_KEY",
            default_model: "claude-sonnet-4-20250514",
        },
        ProviderOption {
            id: "openai",
            name: "OpenAI (GPT)",
            env_var: "OPENAI_API_KEY",
            default_model: "gpt-4.1",
        },
        ProviderOption {
            id: "openrouter",
            name: "OpenRouter (Multi-provider)",
            env_var: "OPENROUTER_API_KEY",
            default_model: "anthropic/claude-sonnet-4",
        },
        ProviderOption {
            id: "ollama",
            name: "Ollama (Local)",
            env_var: "",
            default_model: "llama3.2",
        },
    ];

    // Find default cursor position based on existing provider
    let provider_cursor = existing_provider
        .as_ref()
        .and_then(|p| providers.iter().position(|opt| opt.id == p))
        .unwrap_or(0);

    let provider = Select::new("Model provider:", providers.clone())
        .with_starting_cursor(provider_cursor)
        .with_help_message("Which LLM provider to use")
        .prompt()?;

    // Model display wrapper for nice formatting
    #[derive(Clone)]
    struct ModelChoice {
        id: String,
        name: Option<String>,
        context_window: Option<u64>,
        input_cost: Option<f64>,
        output_cost: Option<f64>,
    }

    impl ModelChoice {
        fn format_pricing(&self) -> Option<String> {
            match (self.input_cost, self.output_cost) {
                (Some(input), Some(output)) => {
                    // Convert per-token to per-million-tokens
                    let input_per_m = input * 1_000_000.0;
                    let output_per_m = output * 1_000_000.0;
                    if input_per_m == 0.0 && output_per_m == 0.0 {
                        Some("free".to_string())
                    } else if input_per_m < 0.01 && output_per_m < 0.01 {
                        // Very cheap, show in cents
                        Some(format!("${:.2}/${:.2}", input_per_m, output_per_m))
                    } else {
                        Some(format!("${:.0}/${:.0}", input_per_m, output_per_m))
                    }
                }
                _ => None,
            }
        }
    }

    impl std::fmt::Display for ModelChoice {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let label = self.name.as_deref().unwrap_or(&self.id);
            let ctx = self.context_window.map(|c| format!("{}k", c / 1000));
            let price = self.format_pricing();

            match (ctx, price) {
                (Some(c), Some(p)) => write!(f, "{} ({}, {})", label, c, p),
                (Some(c), None) => write!(f, "{} ({})", label, c),
                (None, Some(p)) => write!(f, "{} ({})", label, p),
                (None, None) => write!(f, "{}", label),
            }
        }
    }

    // Helper to get models from cache with full metadata
    fn get_cached_models(provider_id: &str) -> Option<Vec<CachedModel>> {
        load_provider_cache(provider_id).map(|c| c.models)
    }

    // 3. Model selection based on provider
    let model: String = match provider.id {
        "anthropic" => {
            let choices: Vec<ModelChoice> = get_cached_models("anthropic")
                .map(|models| {
                    models
                        .into_iter()
                        .map(|m| ModelChoice {
                            id: m.id,
                            name: m.name,
                            context_window: m.context_window,
                            input_cost: m.input_cost,
                            output_cost: m.output_cost,
                        })
                        .collect()
                })
                .unwrap_or_else(|| {
                    vec![
                        ModelChoice {
                            id: "claude-sonnet-4-20250514".into(),
                            name: Some("Claude Sonnet 4".into()),
                            context_window: Some(200000),
                            input_cost: Some(0.000003),
                            output_cost: Some(0.000015),
                        },
                        ModelChoice {
                            id: "claude-opus-4-20250514".into(),
                            name: Some("Claude Opus 4".into()),
                            context_window: Some(200000),
                            input_cost: Some(0.000015),
                            output_cost: Some(0.000075),
                        },
                    ]
                });

            let model_cursor = existing_model
                .as_ref()
                .and_then(|m| choices.iter().position(|c| &c.id == m))
                .unwrap_or(0);

            let help = if load_provider_cache("anthropic").is_some() {
                "From cached provider data"
            } else {
                "Run 'abbot providers refresh' for full list"
            };
            Select::new("Model:", choices)
                .with_starting_cursor(model_cursor)
                .with_help_message(help)
                .prompt()?
                .id
        }
        "openai" => {
            let choices: Vec<ModelChoice> = get_cached_models("openai")
                .map(|models| {
                    // Filter to chat/completion models, skip embeddings etc.
                    models
                        .into_iter()
                        .filter(|m| {
                            m.id.starts_with("gpt-")
                                || m.id.starts_with("o1")
                                || m.id.starts_with("o3")
                        })
                        .map(|m| ModelChoice {
                            id: m.id,
                            name: m.name,
                            context_window: m.context_window,
                            input_cost: m.input_cost,
                            output_cost: m.output_cost,
                        })
                        .collect()
                })
                .unwrap_or_else(|| {
                    vec![
                        ModelChoice {
                            id: "gpt-4.1".into(),
                            name: Some("GPT-4.1".into()),
                            context_window: Some(128000),
                            input_cost: Some(0.000002),
                            output_cost: Some(0.000008),
                        },
                        ModelChoice {
                            id: "gpt-4.1-mini".into(),
                            name: Some("GPT-4.1 Mini".into()),
                            context_window: Some(128000),
                            input_cost: Some(0.0000004),
                            output_cost: Some(0.0000016),
                        },
                        ModelChoice {
                            id: "gpt-4o".into(),
                            name: Some("GPT-4o".into()),
                            context_window: Some(128000),
                            input_cost: Some(0.0000025),
                            output_cost: Some(0.00001),
                        },
                    ]
                });

            let model_cursor = existing_model
                .as_ref()
                .and_then(|m| choices.iter().position(|c| &c.id == m))
                .unwrap_or(0);

            let help = if load_provider_cache("openai").is_some() {
                "From cached provider data"
            } else {
                "Run 'abbot providers refresh' for full list"
            };
            Select::new("Model:", choices)
                .with_starting_cursor(model_cursor)
                .with_help_message(help)
                .prompt()?
                .id
        }
        "openrouter" => {
            // Two-step: first pick model family, then specific model
            let cached = get_cached_models("openrouter");

            // Extract existing family from model (e.g., "anthropic/claude-4" -> "anthropic")
            let existing_family = existing_model
                .as_ref()
                .and_then(|m| m.split('/').next())
                .map(|s| s.to_string());

            if let Some(ref models) = cached {
                // Curated top families + "Other..."
                let top_families = vec![
                    "anthropic",
                    "openai",
                    "google",
                    "meta-llama",
                    "mistralai",
                    "Other...",
                ];

                // Default to existing family if in top list
                let family_cursor = existing_family
                    .as_ref()
                    .and_then(|f| top_families.iter().position(|t| t == f))
                    .unwrap_or(0);

                let family_choice = Select::new("Model family:", top_families.clone())
                    .with_starting_cursor(family_cursor)
                    .with_help_message("Select provider")
                    .prompt()?;

                let family = if family_choice == "Other..." {
                    // Show all families for "Other..."
                    let mut all_families: Vec<String> = models
                        .iter()
                        .filter_map(|m| m.id.split('/').next().map(|s| s.to_string()))
                        .collect::<std::collections::HashSet<_>>()
                        .into_iter()
                        .filter(|f| {
                            !["anthropic", "openai", "google", "meta-llama", "mistralai"]
                                .contains(&f.as_str())
                        })
                        .collect();
                    all_families.sort();

                    // Default to existing family if it's in "Other"
                    let other_cursor = existing_family
                        .as_ref()
                        .and_then(|f| all_families.iter().position(|a| a == f))
                        .unwrap_or(0);

                    Select::new("Model family:", all_families)
                        .with_starting_cursor(other_cursor)
                        .with_help_message("Type to filter")
                        .with_page_size(15)
                        .prompt()?
                } else {
                    family_choice.to_string()
                };

                // Filter models by family and display nicely
                let choices: Vec<ModelChoice> = models
                    .iter()
                    .filter(|m| m.id.starts_with(&format!("{}/", family)))
                    .map(|m| ModelChoice {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        context_window: m.context_window,
                        input_cost: m.input_cost,
                        output_cost: m.output_cost,
                    })
                    .collect();

                // Default to existing model if in this family
                let model_cursor = existing_model
                    .as_ref()
                    .and_then(|m| choices.iter().position(|c| &c.id == m))
                    .unwrap_or(0);

                Select::new("Model:", choices)
                    .with_starting_cursor(model_cursor)
                    .with_help_message("Type to filter")
                    .with_page_size(15)
                    .prompt()?
                    .id
            } else {
                // No cache, use fallback with manual entry
                println!("  (no cached models, run 'abbot providers refresh')");
                let fallback = vec![
                    ModelChoice {
                        id: "anthropic/claude-sonnet-4".into(),
                        name: Some("Claude Sonnet 4".into()),
                        context_window: Some(200000),
                        input_cost: Some(0.000003),
                        output_cost: Some(0.000015),
                    },
                    ModelChoice {
                        id: "openai/gpt-4.1".into(),
                        name: Some("GPT-4.1".into()),
                        context_window: Some(128000),
                        input_cost: Some(0.000002),
                        output_cost: Some(0.000008),
                    },
                    ModelChoice {
                        id: "google/gemini-2.5-pro".into(),
                        name: Some("Gemini 2.5 Pro".into()),
                        context_window: Some(1000000),
                        input_cost: Some(0.00000125),
                        output_cost: Some(0.00001),
                    },
                ];

                let model_cursor = existing_model
                    .as_ref()
                    .and_then(|m| fallback.iter().position(|c| &c.id == m))
                    .unwrap_or(0);

                Select::new("Model:", fallback)
                    .with_starting_cursor(model_cursor)
                    .prompt()?
                    .id
            }
        }
        "ollama" => {
            // For ollama, prefer live query, then cache, then manual entry
            let ollama_models = query_ollama_models();
            let choices: Vec<ModelChoice> = if !ollama_models.is_empty() {
                ollama_models
                    .into_iter()
                    .map(|id| ModelChoice {
                        id,
                        name: None,
                        context_window: None,
                        input_cost: None,
                        output_cost: None,
                    })
                    .collect()
            } else if let Some(cache) = load_provider_cache("ollama") {
                cache
                    .models
                    .into_iter()
                    .map(|m| ModelChoice {
                        id: m.id,
                        name: m.name,
                        context_window: m.context_window,
                        input_cost: m.input_cost,
                        output_cost: m.output_cost,
                    })
                    .collect()
            } else {
                Vec::new()
            };

            if choices.is_empty() {
                println!("  (no ollama models found)");
                let default_model = existing_model.as_deref().unwrap_or("llama3.2");
                Text::new("Model name:")
                    .with_default(default_model)
                    .with_help_message("Enter model name manually")
                    .prompt()?
            } else {
                let model_cursor = existing_model
                    .as_ref()
                    .and_then(|m| choices.iter().position(|c| &c.id == m))
                    .unwrap_or(0);

                Select::new("Model:", choices)
                    .with_starting_cursor(model_cursor)
                    .with_help_message("From 'ollama list'")
                    .prompt()?
                    .id
            }
        }
        _ => provider.default_model.to_string(),
    };

    let full_model_id = format!("{}/{}", provider.id, &model);

    // 4. Pool size
    let pool_sizes = vec!["2 (light)", "4 (default)", "8 (heavy)"];
    let pool_cursor = match existing_pool_size {
        Some(2) => 0,
        Some(8) => 2,
        _ => 1, // default to 4
    };
    let pool_choice = Select::new("Pool size:", pool_sizes)
        .with_starting_cursor(pool_cursor)
        .with_help_message("Number of concurrent workers")
        .prompt()?;

    let pool_size: usize = pool_choice.chars().next().unwrap().to_digit(10).unwrap() as usize;

    // 5. Advanced settings
    let customize = Confirm::new("Customize advanced settings?")
        .with_default(false)
        .with_help_message("Temperature, timeouts, tick intervals")
        .prompt()?;

    // Use existing values or defaults
    let default_head_temp = existing_head_temp.unwrap_or(0.7);
    let default_hand_temp = existing_hand_temp.unwrap_or(0.2);
    let default_head_tick = existing_head_tick.unwrap_or(30);
    let default_timeout = existing_timeout.unwrap_or(300);

    let (head_temp, hand_temp, head_tick, task_timeout) = if customize {
        let temps = vec![
            "0.2 (precise)",
            "0.5 (balanced)",
            "0.7 (creative)",
            "1.0 (wild)",
        ];

        let head_temp_cursor = match default_head_temp {
            t if t <= 0.2 => 0,
            t if t <= 0.5 => 1,
            t if t <= 0.7 => 2,
            _ => 3,
        };
        let head_temp_choice = Select::new("Head temperature:", temps.clone())
            .with_starting_cursor(head_temp_cursor)
            .with_help_message("Higher = more creative responses")
            .prompt()?;

        let hand_temp_cursor = match default_hand_temp {
            t if t <= 0.2 => 0,
            t if t <= 0.5 => 1,
            t if t <= 0.7 => 2,
            _ => 3,
        };
        let hand_temp_choice = Select::new("Hand temperature:", temps)
            .with_starting_cursor(hand_temp_cursor)
            .with_help_message("Lower = more precise tool use")
            .prompt()?;

        let head_temp: f32 = head_temp_choice[..3].parse().unwrap();
        let hand_temp: f32 = hand_temp_choice[..3].parse().unwrap();

        let ticks = vec!["15 (responsive)", "30 (default)", "60 (relaxed)"];
        let tick_cursor = match default_head_tick {
            15 => 0,
            60 => 2,
            _ => 1,
        };
        let tick_choice = Select::new("Head heartbeat (seconds):", ticks)
            .with_starting_cursor(tick_cursor)
            .prompt()?;

        let head_tick: u64 = tick_choice
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();

        let timeouts = vec!["120 (fast)", "300 (default)", "600 (patient)"];
        let timeout_cursor = match default_timeout {
            120 => 0,
            600 => 2,
            _ => 1,
        };
        let timeout_choice = Select::new("Task timeout (seconds):", timeouts)
            .with_starting_cursor(timeout_cursor)
            .prompt()?;

        let task_timeout: u64 = timeout_choice
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();

        (head_temp, hand_temp, head_tick, task_timeout)
    } else {
        (
            default_head_temp,
            default_hand_temp,
            default_head_tick,
            default_timeout,
        )
    };

    // Generate config
    let config_content = format!(
        r#"# Abbot configuration
# Generated by: abbot init

workspace = "{workspace}"

[model]
provider = "{provider}"
model = "{model}"

[head]
model = "{full_model}"
temperature = {head_temp}
heartbeat_tick = {head_tick}
debounce_ms = 500

[hand]
model = "{full_model}"
temperature = {hand_temp}
max_iters = 24

[mind]
model = "{full_model}"
tick_interval = 60

[pool]
size = {pool_size}
timeout_secs = {task_timeout}
"#,
        workspace = workspace_path,
        provider = provider.id,
        model = model,
        full_model = full_model_id,
        head_temp = head_temp,
        hand_temp = hand_temp,
        head_tick = head_tick,
        pool_size = pool_size,
        task_timeout = task_timeout,
    );

    let models_content = r#"# Model definitions
# Format: provider/model-name

[[model]]
id = "openai/gpt-4.1"
provider = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
context_window = 128000
supports_tools = true
supports_vision = true

[[model]]
id = "openai/gpt-4.1-mini"
provider = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
context_window = 128000
supports_tools = true
supports_vision = true

[[model]]
id = "openai/gpt-4o"
provider = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
context_window = 128000
supports_tools = true
supports_vision = true

[[model]]
id = "anthropic/claude-sonnet-4-20250514"
provider = "anthropic"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"
context_window = 200000
supports_tools = true
supports_vision = true

[[model]]
id = "anthropic/claude-opus-4-20250514"
provider = "anthropic"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"
context_window = 200000
supports_tools = true
supports_vision = true

[[model]]
id = "ollama/llama3.2"
provider = "ollama"
base_url = "http://localhost:11434/v1"
api_key_env = ""
context_window = 128000
supports_tools = false
supports_vision = false

[[model]]
id = "ollama/llama3.1"
provider = "ollama"
base_url = "http://localhost:11434/v1"
api_key_env = ""
context_window = 128000
supports_tools = false
supports_vision = false

[[model]]
id = "ollama/codellama"
provider = "ollama"
base_url = "http://localhost:11434/v1"
api_key_env = ""
context_window = 16000
supports_tools = false
supports_vision = false

[[model]]
id = "ollama/mistral"
provider = "ollama"
base_url = "http://localhost:11434/v1"
api_key_env = ""
context_window = 32000
supports_tools = false
supports_vision = false
"#;

    // Write config files
    let config_path = default_config_path().unwrap();
    std::fs::write(&config_path, &config_content)?;
    println!("\nCreated {}", config_path.display());

    let models_path = default_models_path().unwrap();
    if force || !models_path.exists() {
        std::fs::write(&models_path, models_content)?;
        println!("Created {}", models_path.display());
    }

    // Create workspace directory if needed
    let ws_path = Path::new(&workspace_path);
    if !ws_path.exists() {
        std::fs::create_dir_all(ws_path)?;
        println!("Created {}", workspace_path);
    }

    println!("\nConfiguration complete!");

    // API key setup
    if !provider.env_var.is_empty() {
        // Check what keys exist where
        let env_key_before_load = std::env::var(provider.env_var).ok();
        load_api_keys(); // Load keys.env (sets env vars)
        let key_after_load = std::env::var(provider.env_var).ok();

        // Determine if key came from keys.env or was already in environment
        let in_keys_env = key_after_load.is_some() && key_after_load != env_key_before_load;
        let in_shell_env = env_key_before_load.is_some();

        if in_keys_env {
            // Already in keys.env
            println!("\n{} already configured in keys.env.", provider.env_var);
        } else if in_shell_env {
            // Key exists in shell environment but not in keys.env
            let shell_key = env_key_before_load.unwrap();
            let masked = if shell_key.len() > 8 {
                format!(
                    "{}...{}",
                    &shell_key[..4],
                    &shell_key[shell_key.len() - 4..]
                )
            } else {
                "****".to_string()
            };

            let choices = vec![
                format!("Use current key ({})", masked),
                "Enter a different key".to_string(),
                "Skip (don't save to keys.env)".to_string(),
            ];

            let choice = Select::new(
                &format!(
                    "{} found in environment. Save to keys.env?",
                    provider.env_var
                ),
                choices,
            )
            .with_help_message("keys.env is used by the daemon, separate from your shell")
            .prompt()?;

            if choice.starts_with("Use current") {
                save_api_key(provider.env_var, &shell_key)?;
                println!("Saved to ~/.config/abbot/keys.env");
            } else if choice.starts_with("Enter") {
                use inquire::Password;
                let api_key = Password::new(&format!("{}:", provider.env_var))
                    .without_confirmation()
                    .with_help_message("Will be masked, stored securely")
                    .prompt()?;

                if !api_key.is_empty() {
                    save_api_key(provider.env_var, &api_key)?;
                    println!("Saved to ~/.config/abbot/keys.env");
                }
            }
        } else {
            // No key anywhere, prompt to enter one
            let save_key = Confirm::new(&format!("Save {} to keys.env?", provider.env_var))
                .with_default(true)
                .with_help_message("Stored in ~/.config/abbot/keys.env with 0600 permissions")
                .prompt()?;

            if save_key {
                use inquire::Password;
                let api_key = Password::new(&format!("{}:", provider.env_var))
                    .without_confirmation()
                    .with_help_message("Will be masked, stored securely")
                    .prompt()?;

                if !api_key.is_empty() {
                    save_api_key(provider.env_var, &api_key)?;
                    println!("Saved to ~/.config/abbot/keys.env");
                }
            }
        }
    }

    println!("\nRun the daemon:");
    println!("  abbot run");

    Ok(())
}

async fn run_daemon(
    cli: Cli,
    frontend: Option<RunFrontend>,
) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::ems::EmsService;
    use abbot::runtime::app_config::{WorkspacePaths, default_config_path};

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

    // Validate workspace directory exists
    if !workspace.exists() {
        return Err(format!(
            "workspace directory does not exist: {}\nRun 'mkdir -p {}' to create it.",
            workspace.display(),
            workspace.display()
        )
        .into());
    }

    let paths = WorkspacePaths::new(workspace.clone());

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
        tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(log_file))
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt::init();
    }

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

    // Set working directory to workspace/root (the VFS root)
    std::env::set_current_dir(&paths.root)?;

    // Initialize kernel syscall dispatcher with VFS auto-mount
    Kernel::init(&workspace);

    // Expose the effective bind address for bundle context layers.
    // This is safe to surface in debug output and helps the agent reason about localhost vs remote.
    // Safety: we set this once during startup before spawning background services.
    unsafe {
        std::env::set_var("ABBOT_EFFECTIVE_ADDR", &cli.addr);
    }

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

    // Parse autist mode for hands
    let autist_mode = cli
        .autist
        .as_ref()
        .and_then(|s| AutistMode::from_str(s))
        .unwrap_or(AutistMode::None);

    if autist_mode != AutistMode::None {
        tracing::info!(autist = ?autist_mode, "autist mode enabled for hands");
    }

    let mut hand = HandService::new(store.clone(), paths.root.clone(), snapshot.clone())
    .with_autist(autist_mode);
    if let Some(ref ems) = ems_handle {
        hand = hand.with_ems(ems.clone());
    }
    Arc::new(hand).start();

    // Parse generation mode for heads
    let generation_mode = cli
        .generation
        .as_ref()
        .and_then(|s| GenerationMode::from_str(s))
        .unwrap_or(GenerationMode::None);

    if generation_mode != GenerationMode::None {
        tracing::info!(generation = ?generation_mode, "generation mode enabled for heads");
    }

    // Start head pool (kernel need queue dispatches needs to these)
    let head_cfg = HeadConfig::from_env();
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
        )
        .with_generation(generation_mode.clone());
        if let Some(ref ems) = ems_handle {
            head = head.with_ems(ems.clone());
        }
        Arc::new(head).start();
    }

    // Parse fever mode from CLI
    let fever_mode = cli
        .fever
        .as_ref()
        .and_then(|s| FeverMode::from_str(s))
        .unwrap_or(FeverMode::None);

    if fever_mode != FeverMode::None {
        tracing::info!(fever = ?fever_mode, "fever mode enabled");
    }

    Arc::new(
        MindService::new(
            store.clone(),
            DEFAULT_HEAD_ID,
            vec![Scope::main()],
            paths.root.clone(),
        )
        .with_fever(fever_mode)
        .with_conclave_on_boot(cli.conclave),
    )
    .start();

    // Determine web dist path (relative to cargo manifest or executable)
    let web_dist = std::env::var("ABBOT_WEB_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // Try relative to project root
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
        .with_addr(&cli.addr)
        .with_workspace_root(paths.root.clone())
        .with_web_dist(web_dist)
        .spawn();

    // Spawn frontend if requested
    let mut frontend_child: Option<tokio::process::Child> = None;
    if let Some(ref fe) = frontend {
        // Wait for server to be ready
        let health_url = format!("http://{}/health", cli.addr);
        for _ in 0..50 {
            if reqwest::get(&health_url).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        match fe {
            RunFrontend::Opencode { args } => {
                // Update opencode config with current address
                if let Err(e) = update_opencode_config(&cli.addr) {
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
                let base_url = format!("http://{}", cli.addr);
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
                let url = format!("http://{}", cli.addr);
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
        }
    }

    let exit = cli.exit;
    let initial_prompt = cli.prompt.clone();

    if let Some(ref prompt) = initial_prompt {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".into());
        };

        let thread_id = uuid::Uuid::new_v4();
        let scope = Scope::main();

        let _ = k.reply_streams().open(scope.as_str(), thread_id).await;

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
            let mut reply_rx = k.reply_streams().open(scope.as_str(), thread_id).await;
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
    const PROVIDER_ID: &str = "abbot";
    const MODEL_ID: &str = "abbot/default";
    const BASE_URL: &str = "http://localhost:8080/v1";

    match action {
        OpencodeAction::Register => {
            let config_dir = dirs::home_dir()
                .ok_or("could not find home directory")?
                .join(".config")
                .join("opencode");

            std::fs::create_dir_all(&config_dir)?;
            let config_path = config_dir.join("opencode.json");

            // Read existing config or create empty object
            let mut config: serde_json::Value = if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
            } else {
                serde_json::json!({})
            };

            // Ensure provider object exists
            if config.get("provider").is_none() {
                config["provider"] = serde_json::json!({});
            }

            // Add/update abbot provider
            config["provider"][PROVIDER_ID] = serde_json::json!({
                "name": "Abbot",
                "npm": "@ai-sdk/openai-compatible",
                "options": {
                    "baseURL": BASE_URL,
                    "apiKey": "not-required"
                },
                "models": {
                    MODEL_ID: {
                        "name": "Abbot Default",
                        "_launch": true
                    }
                }
            });

            // Write back
            let content = serde_json::to_string_pretty(&config)?;
            std::fs::write(&config_path, content)?;

            println!("Registered abbot provider in {}", config_path.display());
            println!("Run with: abbot opencode run");
        }

        OpencodeAction::Run { args } => {
            let model_arg = format!("{}/{}", PROVIDER_ID, MODEL_ID);

            let mut cmd = std::process::Command::new("opencode");
            cmd.arg("-m").arg(&model_arg);
            cmd.args(&args);

            println!("Running: opencode -m {} {}", model_arg, args.join(" "));

            let status = cmd.status()?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
    }

    Ok(())
}

async fn run_claude(_cli: Cli, action: ClaudeAction) -> Result<(), Box<dyn std::error::Error>> {
    const BASE_URL: &str = "http://127.0.0.1:8080";

    match action {
        ClaudeAction::Run { args } => {
            let mut cmd = std::process::Command::new("claude");
            cmd.env("ANTHROPIC_BASE_URL", BASE_URL);
            cmd.env("ANTHROPIC_API_KEY", "abbot");
            cmd.args(&args);

            println!(
                "Running: ANTHROPIC_BASE_URL={} claude {}",
                BASE_URL,
                args.join(" ")
            );

            let status = cmd.status()?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
    }

    Ok(())
}

fn run_plugin(_cli: Cli, action: PluginAction) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::{PluginManager, atomic_write_file_0600};
    use std::collections::{HashMap, HashSet};

    fn read_plugin_config(path: &std::path::Path) -> HashMap<String, bool> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        };

        let v: toml::Value = match toml::from_str(&raw) {
            Ok(v) => v,
            Err(_) => return HashMap::new(),
        };

        let mut out = HashMap::new();
        let Some(table) = v.as_table() else {
            return out;
        };

        // Legacy format: enabled = ["gh", ...]
        if let Some(enabled) = table.get("enabled").and_then(|v| v.as_array()) {
            for item in enabled {
                if let Some(id) = item.as_str() {
                    out.insert(id.to_string(), true);
                }
            }
        }

        // Current format: [plugin_id] enabled = true
        for (id, value) in table {
            let Some(section) = value.as_table() else {
                continue;
            };
            let enabled = section
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            out.insert(id.to_string(), enabled);
        }

        out
    }

    fn write_plugin_config(
        path: &std::path::Path,
        enabled_ids: &HashSet<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut ids: Vec<String> = enabled_ids.iter().cloned().collect();
        ids.sort();

        let mut table = toml::Table::new();
        for id in ids {
            let mut section = toml::Table::new();
            section.insert("enabled".to_string(), toml::Value::Boolean(true));
            table.insert(id, toml::Value::Table(section));
        }

        let out = toml::to_string(&table)?;
        atomic_write_file_0600(path, &out)?;
        Ok(())
    }

    // Get workspace from config
    let workspace = AppConfig::global()
        .workspace_path()
        .map_err(|e| format!("workspace configuration error: {}", e))?;

    let plugin_config_path = workspace.join("plugins.toml");

    let enabled_map = read_plugin_config(&plugin_config_path);
    let mut enabled: HashSet<String> = enabled_map
        .iter()
        .filter_map(|(k, v)| if *v { Some(k.clone()) } else { None })
        .collect();

    match action {
        PluginAction::Enable { name } => {
            enabled.insert(name.clone());
            write_plugin_config(&plugin_config_path, &enabled)?;
            println!("enabled plugin '{}'", name);
            println!("reboot required to apply");
        }
        PluginAction::Disable { name } => {
            enabled.remove(&name);
            write_plugin_config(&plugin_config_path, &enabled)?;
            println!("disabled plugin '{}'", name);
            println!("reboot required to apply");
        }
        PluginAction::List => {
            let workspace_root = workspace.join("root");

            let mgr = PluginManager::load_for_workspace_root(&workspace_root);
            let catalog = mgr.catalog();

            println!("plugins for workspace: {}", workspace.display());
            if catalog.is_empty() && enabled.is_empty() {
                println!("(no built-in plugins available)");
                return Ok(());
            }

            for p in &catalog {
                let status = if enabled.contains(&p.id) {
                    "ON "
                } else {
                    "OFF"
                };
                let head = format!(
                    "head:{}{}",
                    if p.head_expose { "+" } else { "-" },
                    if p.head_exec { "+" } else { "-" }
                );
                let hand = format!(
                    "hand:{}{}",
                    if p.hand_expose { "+" } else { "-" },
                    if p.hand_exec { "+" } else { "-" }
                );
                println!(
                    "{} {:<12} tool={:<12} {} {}  {}",
                    status, p.id, p.tool_name, head, hand, p.description
                );
            }

            let known_ids: HashSet<String> = catalog.iter().map(|p| p.id.clone()).collect();
            let mut unknown_enabled: Vec<String> = enabled
                .iter()
                .filter(|id| !known_ids.contains(*id))
                .cloned()
                .collect();
            unknown_enabled.sort();

            for id in unknown_enabled {
                println!(
                    "ON  {:<12} tool=<unknown>             (unknown plugin id)",
                    id
                );
            }

            println!("\nrole flags: head=expose/exec, hand=expose/exec (+ = true, - = false)");
        }
    }

    Ok(())
}
