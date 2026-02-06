//! Abbot - Persistent AI Background Daemon
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Abbot is a workspace-scoped AI daemon built on a syscall-driven kernel.
//! The kernel orchestrates three agent types (heads, hands, minds) via a
//! structured syscall interface (chat:*, llm:*, need:*, task:*).
//!
//! WHY a daemon model: Long-running context enables persistent memory, background
//! reflection, and proactive task execution without per-request initialization cost.
//!
//! The syscall refactor (see docs/syscall-refactor-spec.md) establishes:
//! - Turn-based chat lifecycle (chat:message, chat:tool, chat:done)
//! - Internal vs external tool separation (heads/hands vs clients)
//! - Multi-segment turns with external tool resumption
//! - Cancellation and error signaling (chat:cancel, chat:error)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Workspace-scoped: Each abbot instance is tied to a working directory
//! - Agent specialization: Heads (decide), Hands (execute), Minds (reflect)
//! - Syscall-driven: All cross-agent communication flows through kernel syscalls
//! - Protocol adapters: OpenAI-compatible HTTP, web chat SSE, and future protocols
//!   are thin adapters over the same internal turn pipeline
//!
//! TRADE-OFFS
//! ==========
//! - Daemon model requires lifecycle management (start/stop/restart) vs on-demand
//!   serverless execution, trading operational complexity for stateful context
//! - Workspace scoping prevents multi-workspace orchestration but simplifies
//!   security boundaries and reduces cross-talk risk
//!
//! TIMING MODEL
//! ============
//! - Tick: 60 seconds (fixed)
//! - Default sleep: 300 seconds (5 ticks)
//! - Wake debounce: 5 seconds

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use abbot::Scope;
use abbot::history::Store;
use abbot::recall::{Indexer, Ollama, Search, ensure_schema as ensure_recall_schema};
use abbot::runtime::{
    AppConfig, HandService, HeadConfig, HeadService, Kernel, MindLoop, RoomCoordinator,
    ProcService, SessionWriteLocks,
};
use abbot::server::Server;

const DEFAULT_HEAD_ID: &str = "Abbot";

// =============================================================================
// CLI STRUCTURE AND ARGUMENT PARSING
// =============================================================================
//
// WHY Clap-based CLI: Abbot supports many operational modes (daemon, service,
// info, reset, memory management, provider testing). Clap provides structured
// argument parsing and help text generation.

#[derive(Parser, Clone)]
#[command(name = "abbotd")]
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
    /// Show system configuration, status, and health
    Info,
    /// Run the daemon (default), optionally with a frontend
    Run {
        #[command(subcommand)]
        frontend: Option<RunFrontend>,
    },
    /// Manage abbot as a system service
    Service {
        #[command(subcommand)]
        action: ServiceAction,
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
    /// Launch the TUI (assumes daemon is already running)
    Tui {
        /// Additional arguments to pass to abbot-tui
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Query kernel frame logs
    Frames {
        #[command(subcommand)]
        action: FramesAction,
    },
    /// Stream frames from the daemon (websocket)
    Monitor {
        /// Filter by kind/name pattern (e.g., "chat:*", "need:*")
        #[arg(long)]
        filter: Option<String>,
    },
}

#[derive(clap::Subcommand, Clone)]
enum FramesAction {
    /// Get a frame by its UUID
    Get {
        /// Frame UUID
        id: String,
    },
    /// Replay recent frames (excludes tick frames)
    Replay {
        /// Filter by event kind (e.g., chat:user, chat:assistant)
        kind: Option<String>,
        /// Number of frames to return
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Output as readable markdown instead of JSON
        #[arg(long)]
        markdown: bool,
    },
}

#[derive(clap::Subcommand, Clone)]
enum ServiceAction {
    /// Install abbot as a system service (launchd on macOS, systemd on Linux)
    Install,
    /// Uninstall the system service
    Uninstall,
    /// Start the service
    Start,
    /// Stop the service
    Stop,
    /// Show service status
    Status,
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

// =============================================================================
// MAIN ENTRY POINT AND COMMAND DISPATCH
// =============================================================================
//
// WHY command-based dispatch: Abbot supports many operational modes beyond
// just running the daemon (info, service management, memory indexing, provider
// testing). Each command is dispatched to a dedicated handler function.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command.clone() {
        Some(Command::Info) => run_info(cli.clone()).await,
        None | Some(Command::Run { frontend: None }) => run_daemon(cli, None, None).await,
        Some(Command::Run {
            frontend: Some(RunFrontend::Prompt { prompt }),
        }) => run_daemon(cli, None, Some(prompt)).await,
        Some(Command::Run { frontend: Some(f) }) => run_daemon(cli, Some(f), None).await,
        Some(Command::Service { action }) => run_service(action),
        Some(Command::Reset { force, config }) => run_reset(cli.clone(), force, config),
        Some(Command::Memory { action }) => run_memory(cli.clone(), action.clone()).await,
        Some(Command::Plugin { action }) => run_plugin(cli.clone(), action.clone()),
        Some(Command::Providers { action }) => run_providers(action.clone()).await,
        Some(Command::Tui { args }) => run_tui(cli.clone(), args),
        Some(Command::Frames { action }) => run_frames(cli.clone(), action.clone()),
        Some(Command::Monitor { filter }) => run_monitor(cli.clone(), filter).await,
    }
}

// =============================================================================
// INFO COMMAND
// =============================================================================
//
// WHY info command: Displays system configuration, workspace paths, database
// sizes, and provider health. Essential for debugging and operational visibility.

async fn run_info(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{WorkspacePaths, config_dir, default_config_path};

    load_api_keys();

    // Initialize config
    if let Some(ref path) = cli.config {
        AppConfig::init(path);
    } else if let Some(path) = default_config_path() {
        if path.exists() {
            AppConfig::init(&path);
        } else {
            AppConfig::init_default();
        }
    } else {
        AppConfig::init_default();
    }

    let config = AppConfig::global();
    let config_path = cli.config.clone().or_else(default_config_path);
    let keys_path = config_dir().map(|d| d.join("keys.env"));

    fn format_size(bytes: u64) -> String {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }

    fn file_size(path: &std::path::Path) -> String {
        match std::fs::metadata(path) {
            Ok(m) => format_size(m.len()),
            Err(_) => "-".to_string(),
        }
    }

    fn file_preview(path: &std::path::Path, max_lines: usize) -> Option<String> {
        let content = std::fs::read_to_string(path).ok()?;
        let lines: Vec<&str> = content.lines().take(max_lines).collect();
        if lines.is_empty() {
            return None;
        }
        Some(lines.join("\n"))
    }

    fn check_mark(ok: bool) -> &'static str {
        if ok { "[x]" } else { "[ ]" }
    }

    // === Paths ===
    println!("# Abbot System Info\n");

    println!("## Paths\n");
    if let Some(ref p) = config_path {
        println!("- {} config: `{}`", check_mark(p.exists()), p.display());
    }
    if let Some(ref p) = keys_path {
        println!("- {} keys: `{}`", check_mark(p.exists()), p.display());
    }
    match config.workspace_path() {
        Ok(ws) => println!("- {} workspace: `{}`", check_mark(ws.exists()), ws.display()),
        Err(e) => println!("- [ ] workspace: error - {}", e),
    }
    println!();

    // === Databases ===
    if let Ok(workspace) = config.workspace_path() {
        let paths = WorkspacePaths::new(workspace);

        println!("## Databases\n");
        println!("| Database | Size | Purpose |");
        println!("|----------|------|---------|");
        println!("| store.db | {} | conversations |", file_size(&paths.store_db));
        println!("| ems.db | {} | entities |", file_size(&paths.ems_db));
        println!("| frames.db | {} | frame history |", file_size(&paths.frames_db));
        println!("| recall.db | {} | memory embeddings |", file_size(&paths.recall_db));
        println!();

        // === Memory ===
        let self_path = paths.mind.join("self.md");
        let memory_path = paths.mind.join("memory.md");

        if self_path.exists() || memory_path.exists() {
            println!("## Memory\n");

            if self_path.exists() {
                println!("**self.md** ({})", file_size(&self_path));
                if let Some(preview) = file_preview(&self_path, 4) {
                    println!("```");
                    println!("{}", preview);
                    println!("```");
                }
                println!();
            }

            if memory_path.exists() {
                println!("**memory.md** ({})", file_size(&memory_path));
                if let Some(preview) = file_preview(&memory_path, 4) {
                    println!("```");
                    println!("{}", preview);
                    println!("```");
                }
                println!();
            }
        }
    }

    // === Agents ===
    println!("## Agents\n");
    println!("| Agent | Model | Temp | Other |");
    println!("|-------|-------|------|-------|");
    println!("| head | {} | {} | pool: {} |",
        config.head.llm.model.as_deref().unwrap_or("-"),
        config.head.llm.temperature.map(|t| t.to_string()).unwrap_or("-".into()),
        config.pool.size.unwrap_or(4),
    );
    println!("| hand | {} | {} | max_iters: {} |",
        config.hand.llm.model.as_deref().unwrap_or("-"),
        config.hand.llm.temperature.map(|t| t.to_string()).unwrap_or("-".into()),
        config.hand.max_iters.unwrap_or(24),
    );
    println!("| mind | {} | {} | tick: {}s |",
        config.mind.llm.model.as_deref().unwrap_or("-"),
        config.mind.llm.temperature.map(|t| t.to_string()).unwrap_or("-".into()),
        config.mind.tick_interval.unwrap_or(60),
    );
    println!();

    // === Provider Tests ===
    println!("## Providers\n");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    // Collect unique models from config
    let mut models_to_test: Vec<&str> = Vec::new();
    if let Some(ref m) = config.head.llm.model {
        if !models_to_test.contains(&m.as_str()) {
            models_to_test.push(m.as_str());
        }
    }
    if let Some(ref m) = config.hand.llm.model {
        if !models_to_test.contains(&m.as_str()) {
            models_to_test.push(m.as_str());
        }
    }
    if let Some(ref m) = config.mind.llm.model {
        if !models_to_test.contains(&m.as_str()) {
            models_to_test.push(m.as_str());
        }
    }

    // Test each configured provider
    let mut tested_providers: Vec<&str> = Vec::new();
    for model_id in &models_to_test {
        let provider = model_id.split('/').next().unwrap_or("");
        if provider.is_empty() || tested_providers.contains(&provider) {
            continue;
        }
        tested_providers.push(provider);

        let provider_config = config.providers.get(provider);
        let base_url = provider_config
            .and_then(|p| p.base_url.as_deref())
            .unwrap_or(match provider {
                "openrouter" => "https://openrouter.ai/api/v1",
                "anthropic" => "https://api.anthropic.com/v1",
                "openai" => "https://api.openai.com/v1",
                "ollama" => "http://localhost:11434/v1",
                _ => "",
            });

        let env_var = provider_config
            .and_then(|p| p.api_key_env.as_deref())
            .unwrap_or(match provider {
                "anthropic" => "ANTHROPIC_API_KEY",
                "openai" => "OPENAI_API_KEY",
                "openrouter" => "OPENROUTER_API_KEY",
                _ => "",
            });

        let api_key = if !env_var.is_empty() {
            std::env::var(env_var).ok()
        } else {
            None
        };

        let status = match provider {
            "openrouter" => test_openrouter(&client, base_url, api_key.as_deref()).await,
            "anthropic" => test_anthropic(&client, base_url, api_key.as_deref()).await,
            "openai" => test_openai(&client, base_url, api_key.as_deref()).await,
            "ollama" => test_ollama(&client, base_url).await,
            _ => "unknown provider".to_string(),
        };

        let icon = if status == "ok" { "[x]" } else { "[ ]" };
        if status == "ok" {
            println!("- {} **{}**: `{}`", icon, provider, base_url);
        } else {
            println!("- {} **{}**: `{}` - {}", icon, provider, base_url, status);
        }
    }

    if tested_providers.is_empty() {
        println!("- [ ] (no providers configured)");
    }

    println!();
    Ok(())
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

    // Track which section we're in to only update model in relevant sections
    let model_sections = ["[head]", "[hand]", "[mind]", "[prompt_cache]"];
    let mut in_model_section = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Check if entering a new section
        if trimmed.starts_with('[') {
            in_model_section = model_sections.iter().any(|s| trimmed.starts_with(s));
        }

        // Update model lines only within model-related sections
        if in_model_section && trimmed.starts_with("model = ") {
            // Preserve leading whitespace
            let indent = line.len() - line.trim_start().len();
            let whitespace = &line[..indent];
            new_lines.push(format!("{}model = \"{}\"", whitespace, model));
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

fn anthropic_model_info(id: &str) -> (Option<u64>, Option<f64>, Option<f64>) {
    // Pricing per token (divide by 1M for per-token cost)
    // context_window, input_cost_per_1m, output_cost_per_1m
    match id {
        // Claude 4 models
        "claude-sonnet-4-20250514" | "claude-sonnet-4-latest" => {
            (Some(200_000), Some(3.0 / 1_000_000.0), Some(15.0 / 1_000_000.0))
        }
        // Claude 3.5 models
        "claude-3-5-sonnet-20241022" | "claude-3-5-sonnet-latest" | "claude-3-5-sonnet-20240620" => {
            (Some(200_000), Some(3.0 / 1_000_000.0), Some(15.0 / 1_000_000.0))
        }
        "claude-3-5-haiku-20241022" | "claude-3-5-haiku-latest" => {
            (Some(200_000), Some(0.80 / 1_000_000.0), Some(4.0 / 1_000_000.0))
        }
        // Claude 3 models
        "claude-3-opus-20240229" | "claude-3-opus-latest" => {
            (Some(200_000), Some(15.0 / 1_000_000.0), Some(75.0 / 1_000_000.0))
        }
        "claude-3-sonnet-20240229" => {
            (Some(200_000), Some(3.0 / 1_000_000.0), Some(15.0 / 1_000_000.0))
        }
        "claude-3-haiku-20240307" => {
            (Some(200_000), Some(0.25 / 1_000_000.0), Some(1.25 / 1_000_000.0))
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

// =============================================================================
// PROVIDER MANAGEMENT COMMANDS
// =============================================================================
//
// WHY provider commands: Abbot supports multiple LLM providers (Anthropic, OpenAI,
// OpenRouter, Ollama). These commands enable refreshing model catalogs, testing
// connectivity, and listing available models.

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

        ProvidersAction::Models { provider, limit } => {
            let provider = provider.to_lowercase();

            fn format_price(cost: Option<f64>) -> String {
                match cost {
                    None => "-".to_string(),
                    Some(c) if c == 0.0 => "free".to_string(),
                    Some(c) => format!("${:.2}", c * 1_000_000.0),
                }
            }

            match load_provider_cache(&provider) {
                Some(cache) => {
                    println!("Models from {} ({}):\n", provider, cache.fetched_at);
                    for m in cache.models.iter().take(limit) {
                        let price_info = format!(
                            "{} / {}",
                            format_price(m.input_cost),
                            format_price(m.output_cost)
                        );
                        let ctx = m.context_window
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
                        println!("\n  ... and {} more (use --limit to show more)", cache.models.len() - limit);
                    }
                    println!("\nUse: abbot providers use {}/{}", provider, "<model>");
                }
                None => {
                    println!("No cache for '{}'. Run: abbot providers refresh", provider);
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

            // Open browser
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(key_url).spawn();
            #[cfg(target_os = "linux")]
            let _ = std::process::Command::new("xdg-open").arg(key_url).spawn();
            #[cfg(target_os = "windows")]
            let _ = std::process::Command::new("cmd").args(["/C", "start", key_url]).spawn();

            println!("Create a new API key, then paste it here.\n");

            let api_key = Password::new(&format!("{}:", env_var))
                .without_confirmation()
                .prompt()?;

            if api_key.is_empty() {
                println!("No key provided, aborting.");
                return Ok(());
            }

            save_api_key(env_var, &api_key)?;
            unsafe { std::env::set_var(env_var, &api_key); }
            println!("Saved to ~/.config/abbot/keys.env\n");

            // Refresh provider cache
            print!("Fetching models... ");
            match refresh_provider(&provider).await {
                Ok(cache) => println!("{} models cached", cache.models.len()),
                Err(e) => println!("error - {}", e),
            }

            // Test connectivity
            print!("Testing connection... ");
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;

            let status = match provider.as_str() {
                "anthropic" => test_anthropic(&client, "https://api.anthropic.com/v1", Some(&api_key)).await,
                "openai" => test_openai(&client, "https://api.openai.com/v1", Some(&api_key)).await,
                "openrouter" => test_openrouter(&client, "https://openrouter.ai/api/v1", Some(&api_key)).await,
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

        ProvidersAction::Test => {
            use abbot::runtime::app_config::default_config_path;

            println!("Testing provider configurations...\n");

            // Load config to get provider settings
            if let Some(path) = default_config_path() {
                if path.exists() {
                    AppConfig::init(&path);
                }
            }

            let config = AppConfig::global();
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;

            let providers = [
                ("openrouter", "OPENROUTER_API_KEY", true),
                ("anthropic", "ANTHROPIC_API_KEY", true),
                ("openai", "OPENAI_API_KEY", true),
                ("ollama", "", false),
            ];

            let mut results: Vec<(&str, &str, String)> = Vec::new();

            for (name, env_var, needs_key) in providers {
                let provider_config = config.providers.get(name);
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
                    "openai" => {
                        test_openai(&client, base_url, api_key.as_deref()).await
                    }
                    "ollama" => {
                        test_ollama(&client, base_url).await
                    }
                    _ => "unknown provider".to_string(),
                };

                results.push((name, base_url, status));
            }

            // Print results
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

            // Validate model ID format
            if !model.contains('/') {
                println!("Error: model must be in provider/model format (e.g., anthropic/claude-3-5-haiku-latest)");
                return Ok(());
            }

            let provider = model.split('/').next().unwrap_or("");
            let known_providers = ["anthropic", "openai", "openrouter", "ollama"];
            if !known_providers.contains(&provider) {
                println!("Warning: unknown provider '{}'. Known providers: {:?}", provider, known_providers);
            }

            update_config_model(model)?;
            println!("Updated all model configs to: {}", model);
            println!("\nRestart abbot for changes to take effect.");
        }
    }

    Ok(())
}

async fn test_openrouter(
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

async fn test_anthropic(
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

async fn test_openai(
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

async fn test_ollama(client: &reqwest::Client, base_url: &str) -> String {
    // Ollama uses /api/tags for listing models, not OpenAI-compatible /v1/models
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

fn run_service(action: ServiceAction) -> Result<(), Box<dyn std::error::Error>> {
    let service_name = "com.abbot.daemon";

    // Get the path to the current abbot binary
    let abbot_bin = std::env::current_exe()?;

    #[cfg(target_os = "macos")]
    {
        let plist_dir = dirs::home_dir()
            .ok_or("could not find home directory")?
            .join("Library/LaunchAgents");
        let plist_path = plist_dir.join(format!("{}.plist", service_name));

        match action {
            ServiceAction::Install => {
                std::fs::create_dir_all(&plist_dir)?;

                let plist_content = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{service_name}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{abbot_bin}</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/abbot.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/abbot.stderr.log</string>
</dict>
</plist>
"#, service_name = service_name, abbot_bin = abbot_bin.display());

                std::fs::write(&plist_path, plist_content)?;
                println!("Installed service: {}", plist_path.display());
                println!("\nTo start: abbot service start");
            }

            ServiceAction::Uninstall => {
                // Stop first if running
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap_or("")])
                    .output();

                if plist_path.exists() {
                    std::fs::remove_file(&plist_path)?;
                    println!("Uninstalled service: {}", plist_path.display());
                } else {
                    println!("Service not installed");
                }
            }

            ServiceAction::Start => {
                if !plist_path.exists() {
                    println!("Service not installed. Run: abbot service install");
                    return Ok(());
                }

                let output = std::process::Command::new("launchctl")
                    .args(["load", plist_path.to_str().unwrap_or("")])
                    .output()?;

                if output.status.success() {
                    println!("Service started");
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.contains("already loaded") {
                        println!("Service already running");
                    } else {
                        println!("Failed to start: {}", stderr);
                    }
                }
            }

            ServiceAction::Stop => {
                if !plist_path.exists() {
                    println!("Service not installed");
                    return Ok(());
                }

                let output = std::process::Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap_or("")])
                    .output()?;

                if output.status.success() {
                    println!("Service stopped");
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("Failed to stop: {}", stderr);
                }
            }

            ServiceAction::Status => {
                if !plist_path.exists() {
                    println!("Service not installed");
                    return Ok(());
                }

                let output = std::process::Command::new("launchctl")
                    .args(["list", service_name])
                    .output()?;

                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    // Parse launchctl list output: PID Status Label
                    let parts: Vec<&str> = stdout.trim().split_whitespace().collect();
                    if parts.len() >= 3 {
                        let pid = parts[0];
                        let status = parts[1];
                        if pid == "-" {
                            println!("Service: **stopped**");
                            println!("Exit status: {}", status);
                        } else {
                            println!("Service: **running**");
                            println!("PID: {}", pid);
                        }
                    } else {
                        println!("Service: **running**");
                    }
                } else {
                    println!("Service: **stopped** (not loaded)");
                }

                println!("Plist: `{}`", plist_path.display());
                println!("Binary: `{}`", abbot_bin.display());
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let systemd_dir = dirs::home_dir()
            .ok_or("could not find home directory")?
            .join(".config/systemd/user");
        let unit_path = systemd_dir.join("abbot.service");

        match action {
            ServiceAction::Install => {
                std::fs::create_dir_all(&systemd_dir)?;

                let unit_content = format!(r#"[Unit]
Description=Abbot AI Daemon
After=network.target

[Service]
Type=simple
ExecStart={abbot_bin} run
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
"#, abbot_bin = abbot_bin.display());

                std::fs::write(&unit_path, unit_content)?;

                // Reload systemd
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .output();

                println!("Installed service: {}", unit_path.display());
                println!("\nTo start: abbot service start");
                println!("To enable on boot: systemctl --user enable abbot");
            }

            ServiceAction::Uninstall => {
                // Stop and disable first
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "stop", "abbot"])
                    .output();
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "disable", "abbot"])
                    .output();

                if unit_path.exists() {
                    std::fs::remove_file(&unit_path)?;

                    // Reload systemd
                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "daemon-reload"])
                        .output();

                    println!("Uninstalled service: {}", unit_path.display());
                } else {
                    println!("Service not installed");
                }
            }

            ServiceAction::Start => {
                if !unit_path.exists() {
                    println!("Service not installed. Run: abbot service install");
                    return Ok(());
                }

                let output = std::process::Command::new("systemctl")
                    .args(["--user", "start", "abbot"])
                    .output()?;

                if output.status.success() {
                    println!("Service started");
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("Failed to start: {}", stderr);
                }
            }

            ServiceAction::Stop => {
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "stop", "abbot"])
                    .output()?;

                if output.status.success() {
                    println!("Service stopped");
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("Failed to stop: {}", stderr);
                }
            }

            ServiceAction::Status => {
                if !unit_path.exists() {
                    println!("Service not installed");
                    return Ok(());
                }

                let output = std::process::Command::new("systemctl")
                    .args(["--user", "status", "abbot", "--no-pager"])
                    .output()?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                println!("{}", stdout);
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = action;
        println!("Service management not supported on this platform");
        println!("Supported: macOS (launchd), Linux (systemd)");
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
        println!("Run 'abbot run' to regenerate config with defaults.");
    }

    Ok(())
}

// =============================================================================
// DAEMON RUNTIME
// =============================================================================
//
// WHY daemon mode: The core operational mode for Abbot. Initializes the kernel,
// starts agent services (heads, hands, minds), and runs the HTTP/websocket server
// for client connections.
//
// The syscall refactor establishes a turn-based chat lifecycle. The daemon:
// - Accepts chat:message ingress from clients (OpenAI-compatible, web chat)
// - Routes to heads via need:enqueue syscall
// - Streams responses via turn streams until chat:done
// - Supports external tool calls with multi-segment turn resumption
//
// This function handles initialization, service lifecycle, and graceful shutdown.

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
    let frames_db_path = paths.frames_db.clone();

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

    // Local TUI frame stream over Unix domain socket.
    #[cfg(unix)]
    {
        let sock = paths.frames_sock.clone();
        tokio::spawn(async move {
            if let Err(e) = abbot::runtime::serve_frames_uds(sock).await {
                tracing::warn!(error = %e, "frames UDS server failed");
            }
        });
    }

    let store = Arc::new(Store::open(&db_path)?);
    tracing::debug!(db = %db_path.display(), "database opened");

    if let Some(k) = Kernel::get() {
        k.set_store(store.clone());
    }

    // Kernel frame store.
    match abbot::kernel::FrameStore::open(&frames_db_path) {
        Ok(store) => {
            if let Some(k) = Kernel::get() {
                k.set_frames(store).await;
            }
            tracing::debug!(db = %frames_db_path.display(), "frames database opened");
        }
        Err(e) => {
            tracing::warn!(error = %e, db = %frames_db_path.display(), "failed to open frames database");
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

    let coordinator = RoomCoordinator::new(
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![Scope::main()],
        paths.root.clone(),
    )
    .with_conclave_on_boot(cli.conclave);

    Arc::new(coordinator).start();

    let mind_loop = MindLoop::new(store.clone(), paths.root.clone());
    Arc::new(mind_loop).start();

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

        // Best-effort chat ingress (logs + enqueues need internally).
        let dispatcher = k.dispatcher().await;
        let req = abbot::kernel::Frame::req(
            "chat:message",
            serde_json::json!({
                "scope": scope.as_str(),
                "reply_to": thread_id.to_string(),
                "content": prompt,
            }),
        )
        .with_actor("user");
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
                    abbot::kernel::FrameOp::Done | abbot::kernel::FrameOp::Error
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

// =============================================================================
// MEMORY MANAGEMENT COMMANDS
// =============================================================================
//
// WHY memory commands: Abbot's recall system indexes transcript files for
// semantic search. These commands enable indexing, search testing, and wiping
// the memory database for fresh starts.

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

fn run_tui(cli: Cli, args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::default_config_path;

    // Load config to get server address
    if let Some(ref path) = cli.config {
        AppConfig::init(path);
    } else if let Some(path) = default_config_path() {
        if path.exists() {
            AppConfig::init(&path);
        }
    }

    let bind_addr = cli
        .addr
        .or_else(|| AppConfig::global().server.addr.clone())
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    // Look for abbot-tui in the same directory as the current executable first
    let tui_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("abbot-tui")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("abbot-tui"));

    let mut cmd = std::process::Command::new(&tui_path);
    cmd.arg("--addr").arg(&bind_addr);
    cmd.args(&args);

    let status = cmd.status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn run_frames(cli: Cli, action: FramesAction) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{WorkspacePaths, default_config_path};
    use rusqlite::{Connection, params};

    // Load config to get workspace path
    if let Some(ref path) = cli.config {
        AppConfig::init(path);
    } else if let Some(path) = default_config_path() {
        if path.exists() {
            AppConfig::init(&path);
        }
    }

    let workspace = AppConfig::global()
        .workspace_path()
        .map_err(|e| format!("workspace configuration error: {}", e))?;
    let paths = WorkspacePaths::new(workspace);
    let frames_db_path = paths.frames_db;

    if !frames_db_path.exists() {
        eprintln!("Frames database not found: {}", frames_db_path.display());
        std::process::exit(1);
    }

    let conn = Connection::open(&frames_db_path)?;

    match action {
        FramesAction::Get { id } => {
            let mut stmt = conn.prepare(
                "SELECT frame_json FROM frames WHERE frame_id = ?1 LIMIT 1",
            )?;

            let result: Result<String, _> = stmt.query_row(params![id], |row| row.get(0));

            match result {
                Ok(frame_json) => {
                    let frame: serde_json::Value = serde_json::from_str(&frame_json)?;
                    println!("{}", serde_json::to_string_pretty(&frame)?);
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    eprintln!("Frame not found: {}", id);
                    std::process::exit(1);
                }
                Err(e) => return Err(e.into()),
            }
        }

        FramesAction::Replay { kind, limit, markdown } => {
            let (query, query_params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(ref k) = kind {
                let pattern = k.replace('*', "%");
                let is_pattern = pattern.contains('%');
                if is_pattern {
                    (
                        "SELECT seq, ts_ms, frame_json FROM frames
                         WHERE kind LIKE ?1 OR name LIKE ?1
                         ORDER BY seq DESC
                         LIMIT ?2",
                        vec![Box::new(pattern), Box::new(limit as i64)],
                    )
                } else {
                    (
                        "SELECT seq, ts_ms, frame_json FROM frames
                         WHERE kind = ?1 OR name = ?1
                         ORDER BY seq DESC
                         LIMIT ?2",
                        vec![Box::new(k.clone()), Box::new(limit as i64)],
                    )
                }
            } else {
                (
                    "SELECT seq, ts_ms, frame_json FROM frames
                     WHERE (name IS NULL OR name != 'tick')
                       AND (kind IS NULL OR kind != 'SIGTICK')
                     ORDER BY seq DESC
                     LIMIT ?1",
                    vec![Box::new(limit as i64)],
                )
            };

            let mut stmt = conn.prepare(query)?;
            let params_refs: Vec<&dyn rusqlite::ToSql> = query_params.iter().map(|p| p.as_ref()).collect();
            let mut rows = stmt.query(params_refs.as_slice())?;
            let mut frames: Vec<(i64, i64, serde_json::Value)> = Vec::new();

            while let Some(row) = rows.next()? {
                let seq: i64 = row.get(0)?;
                let ts_ms: i64 = row.get(1)?;
                let frame_json: String = row.get(2)?;
                let frame: serde_json::Value = serde_json::from_str(&frame_json)?;
                frames.push((seq, ts_ms, frame));
            }

            if markdown {
                frames.reverse();
                for (seq, ts_ms, frame) in &frames {
                    print_frame_markdown(*seq, *ts_ms, frame);
                }
            } else {
                let json_frames: Vec<serde_json::Value> = frames
                    .into_iter()
                    .map(|(seq, ts_ms, frame)| serde_json::json!({
                        "seq": seq,
                        "ts_ms": ts_ms,
                        "frame": frame,
                    }))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_frames)?);
            }
        }
    }

    Ok(())
}

async fn run_monitor(cli: Cli, filter: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::default_config_path;
    use futures_util::StreamExt;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    // Load config to get server address
    if let Some(ref path) = cli.config {
        AppConfig::init(path);
    } else if let Some(path) = default_config_path() {
        if path.exists() {
            AppConfig::init(&path);
        }
    }

    let bind_addr = cli
        .addr
        .or_else(|| AppConfig::global().server.addr.clone())
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let ws_url = format!("ws://{}/ws", bind_addr);

    // Convert filter pattern for matching
    let filter_pattern = filter.as_ref().map(|f| f.replace('*', ""));
    let filter_is_prefix = filter.as_ref().map(|f| f.ends_with('*')).unwrap_or(false);

    eprintln!("Connecting to {}...", ws_url);

    let (ws_stream, _) = connect_async(&ws_url).await?;
    let (_, mut read) = ws_stream.split();

    eprintln!("Connected. Streaming frames (Ctrl+C to stop)\n");

    // Print header
    println!(
        "{:8}  {:6}  {:20}  {:6}  {:16}  {}",
        "TIME", "OP", "NAME", "SCOPE", "ACTOR", "DATA"
    );
    println!("{}", "-".repeat(100));

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let ws_msg: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if ws_msg.get("type").and_then(|t| t.as_str()) != Some("frame") {
                    continue;
                }

                let Some(frame) = ws_msg.get("data") else {
                    continue;
                };

                let op = frame.get("op").and_then(|v| v.as_str()).unwrap_or("-");
                let name = frame.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let actor = frame.get("actor").and_then(|v| v.as_str()).unwrap_or("-");
                let data = frame.get("data");

                // Skip SIGTICK events
                let kind = data
                    .and_then(|d| d.get("kind"))
                    .and_then(|k| k.as_str())
                    .unwrap_or("");
                if kind == "SIGTICK" {
                    continue;
                }

                // Apply filter
                if let Some(ref pattern) = filter_pattern {
                    let matches = if filter_is_prefix {
                        name.starts_with(pattern) || kind.starts_with(pattern)
                    } else {
                        name == filter.as_deref().unwrap_or("") || kind == filter.as_deref().unwrap_or("")
                    };
                    if !matches {
                        continue;
                    }
                }

                // Extract scope
                let scope = data
                    .and_then(|d| d.get("scope"))
                    .and_then(|s| s.as_str())
                    .map(|s| {
                        if let Some(hash) = s.strip_prefix("session/") {
                            format!("@{}", &hash[..4.min(hash.len())])
                        } else {
                            format!("#{}", s)
                        }
                    })
                    .unwrap_or_default();

                // Format data preview
                let data_preview = data
                    .map(|d| {
                        let s = d.to_string();
                        if s.len() > 60 {
                            format!("{}...", &s[..60])
                        } else {
                            s
                        }
                    })
                    .unwrap_or_default();

                let time = chrono::Local::now().format("%H:%M:%S").to_string();

                println!(
                    "{:8}  {:6}  {:20}  {:6}  {:16}  {}",
                    time,
                    op,
                    truncate_str(name, 20),
                    truncate_str(&scope, 6),
                    truncate_str(actor, 16),
                    data_preview
                );
            }
            Ok(Message::Close(_)) => {
                eprintln!("\nConnection closed");
                break;
            }
            Err(e) => {
                eprintln!("\nWebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len - 1])
    }
}

fn truncate_content(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{}...", truncated)
}

fn print_frame_markdown(seq: i64, ts_ms: i64, frame: &serde_json::Value) {
    use chrono::{Local, TimeZone};

    let op = frame["op"].as_str().unwrap_or("");
    let name = frame["name"].as_str().unwrap_or("");
    let data = &frame["data"];

    // kind lives in data.kind for events, frame.kind for some others
    let kind = frame["kind"].as_str()
        .or_else(|| data["kind"].as_str())
        .unwrap_or("");

    // actor lives at frame level for items/reqs, data.data.sender for events
    let actor = frame["actor"].as_str()
        .or_else(|| data["data"]["sender"].as_str())
        .unwrap_or("");

    // Skip noise: operational frames with no diagnostic value
    match (op, name) {
        ("req", "log:append") | ("req", "need:lease") | ("req", "task:lease")
        | ("req", "tick:subscribe") | ("req", "tool:register") => return,
        ("ok", _) | ("done", _) => return,
        _ => {}
    }

    // Skip event-stream duplicates (items already carry the same content)
    if op == "event" && matches!(kind, "chat:user" | "chat:head" | "chat:tool_result" | "thinking" | "chat:reset") {
        return;
    }

    // Skip req chat:* (items carry the same content with actor)
    if op == "req" && matches!(name, "chat:message" | "chat:tool" | "chat:done" | "chat:tool_result") {
        return;
    }

    // Skip item chat:message/chat:tool (item llm:chat already shows these)
    if op == "item" && matches!(name, "chat:message" | "chat:tool") {
        return;
    }

    let ts = Local.timestamp_millis_opt(ts_ms).single();
    let time_str = match ts {
        Some(t) => t.format("%H:%M:%S%.3f").to_string(),
        None => format!("{}ms", ts_ms),
    };

    // Build header: show actor when available, otherwise kind
    let context = if !actor.is_empty() {
        actor.to_string()
    } else if !kind.is_empty() {
        kind.to_string()
    } else {
        String::new()
    };
    if context.is_empty() {
        println!("\n### [{}] #{} {} {}", time_str, seq, op, name);
    } else {
        println!("\n### [{}] #{} {} {} ({})", time_str, seq, op, name, context);
    }

    match (op, name, kind) {
        ("error", _, _) => {
            let code = data["code"].as_str().unwrap_or("UNKNOWN");
            let retryable = data["retryable"].as_bool().unwrap_or(false);
            let msg = data["message"].as_str().unwrap_or("");
            let retry_str = if retryable { "retryable" } else { "fatal" };
            println!("**Error** `{}` ({}) {}", code, retry_str, truncate_content(msg, 200));
        }

        ("event", "llm:chat", "llm:begin") => {
            let model = data["model"].as_str().unwrap_or("?");
            let provider = data["provider"].as_str().unwrap_or("?");
            let n_messages = data["messages"].as_u64().unwrap_or(0);
            let n_tools = data["tools"].as_u64().unwrap_or(0);
            println!("LLM call: **{}** via {} | {} messages, {} tools", model, provider, n_messages, n_tools);
        }

        ("event", "llm:chat", "llm:result") => {
            let usage = &data["usage"];
            let completion = usage["completion_tokens"].as_u64()
                .or_else(|| data["completion_tokens"].as_u64())
                .unwrap_or(0);
            let prompt = usage["prompt_tokens"].as_u64()
                .or_else(|| data["prompt_tokens"].as_u64())
                .unwrap_or(0);
            let total = completion + prompt;
            println!("LLM result: {} completion + {} prompt = **{} total tokens**", completion, prompt, total);
        }

        // item llm:chat: sub-dispatch on data.type
        ("item", "llm:chat", _) => {
            let item_type = data["type"].as_str().unwrap_or("");
            match item_type {
                "thinking" => {
                    let content = data["content"].as_str().unwrap_or("");
                    println!("> *thinking:* {}", truncate_content(content, 600));
                }
                "text_delta" => {
                    let content = data["content"].as_str().unwrap_or("");
                    println!("**{}:** {}", if actor.is_empty() { "Assistant" } else { actor }, truncate_content(content, 600));
                }
                "tool_call" => {
                    let tool_name = data["name"].as_str().unwrap_or("?");
                    let args = if data["arguments"].is_string() {
                        data["arguments"].as_str().unwrap_or("").to_string()
                    } else if data["arguments"].is_object() {
                        serde_json::to_string(&data["arguments"]).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    println!("**Tool call:** `{}` {}", tool_name, truncate_content(&args, 200));
                }
                "tool_result" => {
                    let tool_name = data["name"].as_str().unwrap_or("?");
                    let content = data["content"].as_str().unwrap_or("");
                    println!("**Tool result** (`{}`): {}", tool_name, truncate_content(content, 200));
                }
                _ => {
                    let compact = serde_json::to_string(data).unwrap_or_default();
                    println!("{}", truncate_content(&compact, 300));
                }
            }
        }

        ("event", "mind:conclave", "mind:round_start") => {
            let round = data["round"].as_u64().unwrap_or(0);
            let round_type = data["type"].as_str().unwrap_or("?");
            println!("**Conclave round {}** ({})", round, round_type);
        }

        ("event", "mind:autonomy", "mind:start") => {
            let wake = data["wake"].as_str()
                .or_else(|| data["wake_reason"].as_str())
                .unwrap_or("?");
            println!("**Autonomy meeting started** | wake={}", wake);
        }

        ("item", "chat:done", _) => {
            let reason = data["reason"].as_str()
                .or_else(|| data["stop_reason"].as_str())
                .unwrap_or("?");
            println!("**Turn complete** ({})", reason);
        }

        ("req", "need:enqueue", _) => {
            let priority = data["priority"].as_str().unwrap_or("?");
            let text = data["need"].as_str()
                .or_else(|| data["text"].as_str())
                .unwrap_or("");
            println!("**Need created** ({}): {}", priority, truncate_content(text, 200));
        }

        ("req", "need:fulfill", _) => {
            let id = data["need_id"].as_str()
                .or_else(|| data["id"].as_str())
                .unwrap_or("?");
            let short_id = if id.len() > 8 { &id[..8] } else { id };
            let summary = data["summary"].as_str().unwrap_or("");
            println!("**Need fulfilled** `{}`: {}", short_id, truncate_content(summary, 200));
        }

        ("req", "task:enqueue", _) => {
            let head_id = data["head_id"].as_str().unwrap_or("?");
            let short_head = if head_id.len() > 8 { &head_id[..8] } else { head_id };
            let prompt = data["prompt"].as_str().unwrap_or("");
            let input = data["input"].as_str().unwrap_or("");
            let combined = format!("{} {}", prompt, input);
            println!("**Task dispatched** to {}: {}", short_head, truncate_content(&combined, 200));
        }

        ("req", "task:complete", _) => {
            let ok = data["ok"].as_bool().unwrap_or(false);
            let summary = data["summary"].as_str().unwrap_or("");
            println!("**Task complete** (ok: {}): {}", ok, truncate_content(summary, 200));
        }

        ("req", "llm:chat", _) => {
            let n_messages = data["messages"].as_array().map_or(0, |a| a.len());
            println!("**LLM request**: {} messages", n_messages);
        }

        _ => {
            let compact = serde_json::to_string(data).unwrap_or_default();
            if compact != "null" && compact != "{}" {
                println!("{}", truncate_content(&compact, 300));
            }
        }
    }
}
