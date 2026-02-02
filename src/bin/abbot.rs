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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::sync::RwLock;
use uuid::Uuid;

use abbot::bus::{Message, MessageData, MessageOp, Origin, Scope, respond};
use abbot::history::Store;
use abbot::bus::NeedPriority;
use abbot::runtime::{
    AppConfig, TaskService, HandService, HeadService, MindService, NeedService, StatService, RuntimeBus,
    FeverMode, GenerationMode, AutistMode, HeadConfig, SessionWriteLocks,
    ProcService,
};
use abbot::server::Server;
use abbot::recall::{ensure_schema as ensure_recall_schema, Indexer, Ollama, Search};

const DEFAULT_SANDBOX: &str = "default";
const DEFAULT_HEAD_ID: &str = "Abbot";
const DEFAULT_PING_SCOPE: &str = "ping";

const TICK_SECONDS: u64 = 60;

#[derive(Parser, Clone)]
#[command(name = "abbot")]
#[command(about = "Abbot: persistent AI background daemon", version)]
struct Cli {
    /// Sandbox name (workspace and db stored in ~/.local/abbot/<sandbox>/)
    #[arg(long, env = "ABBOT_SANDBOX", default_value = DEFAULT_SANDBOX)]
    sandbox: String,

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
    /// Initialize Abbot (create config files and default sandbox)
    Init,
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
    /// Manage sandbox mounts (symlinks to external directories)
    Mount {
        #[command(subcommand)]
        action: MountAction,
    },
    /// Manage sandboxes
    Sandbox {
        #[command(subcommand)]
        action: SandboxAction,
    },
    /// Export sandbox history as transcript
    Export {
        /// Name of the sandbox to export (default: from --sandbox flag)
        name: Option<String>,
        /// Output file path (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Manage sandbox plugins (tools)
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
}

#[derive(clap::Subcommand, Clone)]
enum PluginAction {
    /// Enable a plugin for the sandbox
    Enable { name: String },
    /// Disable a plugin for the sandbox
    Disable { name: String },
    /// List available plugins and their sandbox status
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
enum SandboxAction {
    /// Create a new sandbox
    Create {
        /// Name for the sandbox
        name: String,
    },
    /// Clone a git repository into a new sandbox
    Clone {
        /// Git repository URL
        url: String,
        /// Name for the sandbox (default: derived from repo name)
        #[arg(long)]
        name: Option<String>,
    },
    /// List all sandboxes
    List,
    /// Show detailed status of a sandbox
    Status {
        /// Name of the sandbox (default: from --sandbox flag)
        name: Option<String>,
    },
    /// Delete a sandbox and all its data
    Delete {
        /// Name of the sandbox to delete
        name: String,
    },
    /// Reset a sandbox (delete data but keep mounts)
    Reset {
        /// Name of the sandbox to reset
        name: String,
    },
}

#[derive(clap::Subcommand, Clone)]
enum MountAction {
    /// Add a mount (symlink external directory into sandbox)
    Add {
        /// Name for the mount (directory name inside sandbox)
        name: String,
        /// Path to external directory
        path: PathBuf,
    },
    /// Remove a mount
    Remove {
        /// Name of the mount to remove
        name: String,
    },
    /// List all mounts in the sandbox
    List,
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

// Tracks head state for reply chain completion (used by --exit flag)
struct HeadState {
    sleeping: bool,
}

impl HeadState {
    fn new() -> Self {
        Self { sleeping: false }
    }

    fn mark_sleeping(&mut self) {
        self.sleeping = true;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command.clone() {
        None | Some(Command::Run { frontend: None }) => run_daemon(cli, None).await,
        Some(Command::Run { frontend: Some(f) }) => run_daemon(cli, Some(f)).await,
        Some(Command::Init) => run_init(),
        Some(Command::Memory { action }) => run_memory(cli.clone(), action.clone()).await,
        Some(Command::Opencode { action }) => run_opencode(cli.clone(), action.clone()).await,
        Some(Command::Claude { action }) => run_claude(cli.clone(), action.clone()).await,
        Some(Command::Mount { action }) => run_mount(cli.clone(), action.clone()),
        Some(Command::Sandbox { action }) => run_sandbox(cli.clone(), action.clone()),
        Some(Command::Export { name, output }) => run_export(cli.clone(), name.clone(), output.clone()),
        Some(Command::Plugin { action }) => run_plugin(cli.clone(), action.clone()),
    }
}

fn run_init() -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{config_dir, data_dir, default_config_path, default_models_path, sandbox_workspace, sandbox_env, create_sandbox_env};

    println!("Initializing Abbot...\n");

    // Create config directory
    let config_dir = config_dir().ok_or("could not determine config directory")?;
    if !config_dir.exists() {
        std::fs::create_dir_all(&config_dir)?;
        println!("created {}", config_dir.display());
    } else {
        println!("exists  {}", config_dir.display());
    }

    // Create abbot.toml
    let config_path = default_config_path().unwrap();
    if !config_path.exists() {
        let default_config = r#"# Abbot configuration
# See: https://github.com/ianzepp/abbot

[head]
model = "openai/gpt-4.1"
temperature = 0.7
heartbeat_tick = 30
debounce_ms = 500

[hand]
model = "openai/gpt-4.1-mini"
temperature = 0.2
max_iters = 24

[mind]
model = "openai/gpt-4.1"
tick_interval = 60

[pool]
size = 4
timeout_secs = 300
"#;
        std::fs::write(&config_path, default_config)?;
        println!("created {}", config_path.display());
    } else {
        println!("exists  {}", config_path.display());
    }

    // Create models.toml
    let models_path = default_models_path().unwrap();
    if !models_path.exists() {
        let default_models = r#"# Model definitions
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
id = "anthropic/claude-sonnet-4-20250514"
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
"#;
        std::fs::write(&models_path, default_models)?;
        println!("created {}", models_path.display());
    } else {
        println!("exists  {}", models_path.display());
    }

    // Create data directory
    let data_dir = data_dir().ok_or("could not determine data directory")?;
    if !data_dir.exists() {
        std::fs::create_dir_all(&data_dir)?;
        println!("created {}", data_dir.display());
    } else {
        println!("exists  {}", data_dir.display());
    }

    // Create default sandbox
    let default_sandbox = sandbox_workspace("default").unwrap();
    if !default_sandbox.exists() {
        std::fs::create_dir_all(&default_sandbox)?;
        println!("created {}", default_sandbox.display());
    } else {
        println!("exists  {}", default_sandbox.display());
    }

    // Create default sandbox env file
    let default_env = sandbox_env("default").unwrap();
    if create_sandbox_env("default")? {
        println!("created {}", default_env.display());
    } else {
        println!("exists  {}", default_env.display());
    }

    // Create default sandbox mind metadata
    if abbot::runtime::create_sandbox_mind_metadata("default")? {
        if let Some(sandbox_dir) = default_sandbox.parent() {
            println!("created {}/mind/memory.md", sandbox_dir.display());
            println!("created {}/mind/self.md", sandbox_dir.display());
        }
    }

    // Create default sandbox config
    if abbot::runtime::create_sandbox_config("default")? {
        if let Some(config_path) = abbot::runtime::sandbox_config("default") {
            println!("created {}", config_path.display());
        }
    }

    println!("\nAbbot initialized!");
    println!("\nNext steps:");
    println!("  1. Set your API key:  export OPENAI_API_KEY=sk-...");
    println!("  2. Or add it to:      {}", default_env.display());
    println!("  3. Run the daemon:    abbot run");
    println!("  4. Or clone a repo:   abbot sandbox clone <git-url>");

    Ok(())
}

async fn run_daemon(cli: Cli, frontend: Option<RunFrontend>) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{default_config_path, sandbox_workspace, sandbox_db, sandbox_dir, sandbox_recall_db, create_sandbox_env, create_sandbox_mind_metadata, create_sandbox_config, load_sandbox_env};

    // When running with a TUI frontend, redirect logs to a file to avoid corrupting the display
    let is_tui = matches!(frontend, Some(RunFrontend::Opencode { .. }) | Some(RunFrontend::Claude { .. }));
    if is_tui {
        let log_path = sandbox_dir(&cli.sandbox)
            .map(|d| d.join("daemon.log"))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/abbot-daemon.log"));
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

    // Load sandbox env vars before anything else
    match load_sandbox_env(&cli.sandbox) {
        Ok(0) => {}
        Ok(n) => tracing::debug!(sandbox = %cli.sandbox, count = n, "loaded sandbox env vars"),
        Err(e) => tracing::warn!(sandbox = %cli.sandbox, error = %e, "failed to load sandbox env"),
    }

    if let Some(ref path) = cli.config {
        AppConfig::init(path);
        tracing::debug!(config = %path.display(), "loaded config");
    } else if let Some(path) = default_config_path() {
        AppConfig::init(&path);
        tracing::debug!(config = %path.display(), "loaded config");
    } else {
        AppConfig::init_default();
        tracing::warn!("no config file found, using defaults");
    }

    // Resolve sandbox paths
    let workspace_path = sandbox_workspace(&cli.sandbox)
        .ok_or_else(|| "could not determine data directory for sandbox")?;
    let db_path = sandbox_db(&cli.sandbox)
        .ok_or_else(|| "could not determine database path for sandbox")?;
    let recall_db_path = sandbox_recall_db(&cli.sandbox)
        .ok_or_else(|| "could not determine recall database path for sandbox")?;

    // Migrate legacy memory.sqlite -> recall.sqlite if present.
    if !recall_db_path.exists() {
        if let Some(legacy) = abbot::runtime::app_config::sandbox_dir(&cli.sandbox)
            .map(|p| p.join("memory.sqlite"))
        {
            if legacy.exists() {
                let _ = std::fs::rename(&legacy, &recall_db_path);
            }
        }
    }

    // Create sandbox workspace directory if it doesn't exist
    if !workspace_path.exists() {
        std::fs::create_dir_all(&workspace_path)?;
    }

    // Create sandbox metadata files if they don't exist
    create_sandbox_env(&cli.sandbox)?;
    create_sandbox_mind_metadata(&cli.sandbox)?;
    create_sandbox_config(&cli.sandbox)?;

    tracing::info!(
        sandbox = %cli.sandbox,
        workspace = %workspace_path.display(),
        "abbot starting"
    );

    // Set working directory to sandbox workspace
    std::env::set_current_dir(&workspace_path)?;

    // Expose the effective bind address for bundle context layers.
    // This is safe to surface in debug output and helps the agent reason about localhost vs remote.
    // Safety: we set this once during startup before spawning background services.
    unsafe {
        std::env::set_var("ABBOT_EFFECTIVE_ADDR", &cli.addr);
    }

    let store = Arc::new(Store::open(&db_path)?);
    tracing::debug!(db = %db_path.display(), "database opened");

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

    let hub = Arc::new(RwLock::new(abbot::bus::Hub::new()));
    let bus = RuntimeBus::new(hub.clone(), store.clone());
    let proc = ProcService::new().handle();

    let snapshot = abbot::runtime::SnapshotManager::new(workspace_path.clone(), Some(store.clone()));

    let head_scope = Scope::main();
    let head_mail_scope = Scope::head_mail(DEFAULT_HEAD_ID);
    let ping_scope = Scope::from(DEFAULT_PING_SCOPE);

    bus.create_scope(head_scope.clone()).await;
    bus.create_scope(head_mail_scope.clone()).await;
    bus.create_scope(ping_scope.clone()).await;

    let task_service = Arc::new(TaskService::new(bus.clone(), proc.clone()));
    let task_query = task_service.query_handle();
    task_service.start();
    Arc::new(NeedService::new(bus.clone(), proc.clone())).start();
    Arc::new(StatService::new(bus.clone(), store.clone())).start();
    Arc::new(abbot::runtime::RecallFlushService::new(bus.clone(), store.clone(), workspace_path.clone())).start();
    Arc::new(abbot::runtime::IdleMonitorService::new(bus.clone(), workspace_path.clone())).start();

    // Parse autist mode for hands
    let autist_mode = cli.autist
        .as_ref()
        .and_then(|s| AutistMode::from_str(s))
        .unwrap_or(AutistMode::None);

    if autist_mode != AutistMode::None {
        tracing::info!(autist = ?autist_mode, "autist mode enabled for hands");
    }

    Arc::new(HandService::new(bus.clone(), store.clone(), snapshot.clone()).with_autist(autist_mode)).start();

    // Parse generation mode for heads
    let generation_mode = cli.generation
        .as_ref()
        .and_then(|s| GenerationMode::from_str(s))
        .unwrap_or(GenerationMode::None);

    if generation_mode != GenerationMode::None {
        tracing::info!(generation = ?generation_mode, "generation mode enabled for heads");
    }

    // Start head pool (NeedService will dispatch needs to these)
    let head_cfg = HeadConfig::from_env();
    let session_locks = SessionWriteLocks::new();
    tracing::info!(pool_size = head_cfg.pool_size, "starting head pool");
    for i in 0..head_cfg.pool_size {
        let head_id = format!("head-{}", i);
        let head_mail = Scope::head_mail(&head_id);
        bus.create_scope(head_mail.clone()).await;
        Arc::new(HeadService::new(
            bus.clone(),
            store.clone(),
            &head_id,
            vec![head_scope.clone(), head_mail], // include mailbox so head can see task results
            memory_search.clone(),
            snapshot.clone(),
            session_locks.clone(),
            Some(task_query.clone()),
        ).with_generation(generation_mode.clone()))
        .start();
    }

    // Parse fever mode from CLI
    let fever_mode = cli.fever
        .as_ref()
        .and_then(|s| FeverMode::from_str(s))
        .unwrap_or(FeverMode::None);

    if fever_mode != FeverMode::None {
        tracing::info!(fever = ?fever_mode, "fever mode enabled");
    }

    Arc::new(MindService::new(
        bus.clone(),
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![head_scope.clone(), head_mail_scope.clone()],
        workspace_path.clone(),
    ).with_fever(fever_mode).with_conclave_on_boot(cli.conclave))
    .start();

    // Determine web dist path (relative to cargo manifest or executable)
    let web_dist = std::env::var("ABBOT_WEB_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // Try relative to project root
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from(".")));
            manifest_dir.join("web").join("dist")
        });

    Server::new(bus.clone(), store.clone(), DEFAULT_HEAD_ID)
        .with_addr(&cli.addr)
        .with_sandbox_root(workspace_path.clone())
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
                    .arg("-m").arg(model_arg)
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
                let result = std::process::Command::new("cmd").args(["/C", "start", &url]).spawn();

                if let Err(e) = result {
                    tracing::warn!(error = %e, "failed to open browser");
                }
            }
        }
    }

    let exit = cli.exit;
    let initial_prompt = cli.prompt.clone();

    if let Some(ref prompt) = initial_prompt {
        tracing::debug!(prompt = %prompt, "sending initial prompt");
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Publish to main scope for history
        bus.publish(
            respond::chat("user", Scope::main(), prompt)
                .with_origin(Origin::Human),
        )
        .await;

        // Create need for NeedService to dispatch
        let need_id = uuid::Uuid::new_v4().to_string();
        bus.publish(
            respond::need_request(
                "user",
                Scope::main(),
                &need_id,
                "user",
                NeedPriority::Normal,
                prompt,
                "",
            )
            .with_origin(Origin::Human),
        )
        .await;
    }

    tracing::debug!(
        tick_s = TICK_SECONDS,
        exit = exit,
        "heartbeat loop started"
    );

    let mut heads: HashMap<String, HeadState> = HashMap::new();
    heads.insert(DEFAULT_HEAD_ID.to_string(), HeadState::new());

    let mut rx = hub.read().await.subscribe_all();
    let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECONDS));
    let mut tick: u64 = 0;
    let prompt_sent = initial_prompt.is_some();
    let mut pending_chains: HashMap<Option<Uuid>, usize> = HashMap::new();
    let mut task_reply_to: HashMap<String, Option<Uuid>> = HashMap::new();
    let mut done_sent: HashSet<Option<Uuid>> = HashSet::new();
    let mut active_tasks: i64 = 0;
    let mut active_needs: i64 = 0;
    let mut ever_busy: bool = false;
    let mut idle_emitted: bool = false;
    let mut reboot_pending: bool = false;
    let mut reboot_reason: String = String::new();
    let mut reboot_mode: String = "hard".to_string();
    let mut reboot_proposer: String = String::new();

    loop {
        tokio::select! {
            _ = interval.tick() => {
                tick += 1;
                tracing::debug!(tick, "tick");

                // Publish ping for Mind to wake on its interval
                bus.publish(
                    respond::ping("_harness", ping_scope.clone(), tick)
                        .with_origin(Origin::System),
                )
                .await;
            }

            msg = rx.recv() => {
                let Ok(msg) = msg else { continue };

                if msg.op == MessageOp::Event {
                    if let MessageData::Event { kind, payload } = &msg.data {
                        if msg.origin == Origin::System && kind == "reboot_requested" {
                            reboot_pending = true;
                            reboot_reason = payload
                                .get("reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or("reboot requested")
                                .to_string();
                            reboot_mode = payload
                                .get("mode")
                                .and_then(|v| v.as_str())
                                .unwrap_or("hard")
                                .to_string();
                            reboot_proposer = payload
                                .get("proposer")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            tracing::warn!(mode = %reboot_mode, proposer = %reboot_proposer, reason = %reboot_reason, "reboot pending (will apply at idle)");
                        }
                    }
                }

                // Track work-in-flight for idle detection.
                match (&msg.op, &msg.data) {
                    (MessageOp::Task, MessageData::Task(abbot::bus::TaskMsg::Request { .. })) => {
                        active_tasks += 1;
                        ever_busy = true;
                    }
                    (MessageOp::Task, MessageData::Task(abbot::bus::TaskMsg::Result { .. })) => {
                        active_tasks = (active_tasks - 1).max(0);
                    }
                    (MessageOp::Need, MessageData::Need(abbot::bus::NeedMsg::Request { .. })) => {
                        active_needs += 1;
                        ever_busy = true;
                    }
                    (MessageOp::Need, MessageData::Need(abbot::bus::NeedMsg::Fulfilled { .. })) => {
                        active_needs = (active_needs - 1).max(0);
                    }
                    (MessageOp::Need, MessageData::Need(abbot::bus::NeedMsg::Expired { .. })) => {
                        active_needs = (active_needs - 1).max(0);
                    }
                    _ => {}
                }

                let event = handle_message(&msg, &mut heads, tick);

                match event {
                    MessageEvent::HeadSlept => {
                        // Emit Done for any reply chains that have completed (count == 0)
                        let completed: Vec<Option<Uuid>> = pending_chains
                            .iter()
                            .filter(|(_, count)| **count == 0)
                            .map(|(reply_to, _)| *reply_to)
                            .collect();

                        for reply_to in completed {
                            pending_chains.remove(&reply_to);
                            if !done_sent.contains(&reply_to) {
                                done_sent.insert(reply_to);
                                let mut done_msg = respond::done("_harness", Scope::main())
                                    .with_origin(Origin::System);
                                if let Some(id) = reply_to {
                                    done_msg = done_msg.with_reply_to(id);
                                }
                                tracing::debug!(reply_to = ?reply_to, "emitting Done for completed chain");
                                bus.publish(done_msg).await;
                            }
                        }

                        // Idle is emitted outside of this match when work counters reach zero.
                    }
                    MessageEvent::TaskRequested { task_id, reply_to } => {
                        task_reply_to.insert(task_id, reply_to);
                        *pending_chains.entry(reply_to).or_insert(0) += 1;
                    }
                    MessageEvent::TaskCompleted { task_id } => {
                        if let Some(reply_to) = task_reply_to.remove(&task_id) {
                            if let Some(count) = pending_chains.get_mut(&reply_to) {
                                *count = count.saturating_sub(1);
                            }
                        }
                    }
                    MessageEvent::UserMessage { msg_id } => {
                        // Track the chain even if it never creates tasks.
                        pending_chains.entry(Some(msg_id)).or_insert(0);
                    }
                    MessageEvent::None => {}
                }

                // Emit Idle when all tracked work is complete.
                if active_tasks == 0 && active_needs == 0 {
                    if reboot_pending {
                        reboot_pending = false;
                        let epoch = abbot::runtime::bump_reboot_epoch();
                        tracing::warn!(epoch, mode = %reboot_mode, proposer = %reboot_proposer, reason = %reboot_reason, "applying collective reboot (idle boundary)");
                        bus.publish(
                            respond::event(
                                "_harness",
                                Scope::main(),
                                "collective_reboot",
                                serde_json::json!({
                                    "epoch": epoch,
                                    "mode": reboot_mode,
                                    "reason": reboot_reason,
                                    "proposer": reboot_proposer,
                                }),
                            )
                            .with_origin(Origin::System),
                        )
                        .await;
                    }

                    if ever_busy && !idle_emitted {
                        tracing::debug!("emitting Idle (system fully idle)");
                        bus.publish(
                            respond::idle("_harness", Scope::main())
                                .with_origin(Origin::System),
                        )
                        .await;

                        idle_emitted = true;

                        if exit && prompt_sent {
                            tracing::info!("exiting (--exit mode)");
                            break;
                        }
                    }
                } else {
                    idle_emitted = false;
                }
            }

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

enum MessageEvent {
    None,
    HeadSlept,
    TaskRequested { task_id: String, reply_to: Option<Uuid> },
    TaskCompleted { task_id: String },
    UserMessage { msg_id: Uuid },
}

fn handle_message(msg: &Message, heads: &mut HashMap<String, HeadState>, _current_tick: u64) -> MessageEvent {
    match (&msg.op, &msg.data) {
        (MessageOp::Sleep, MessageData::Sleep { seconds }) => {
            if msg.origin == Origin::Head {
                if let Some(state) = heads.get_mut(&msg.sender) {
                    tracing::info!(head = %msg.sender, seconds, "head sleeping");
                    state.mark_sleeping();
                    return MessageEvent::HeadSlept;
                }
            }
        }

        (MessageOp::Task, MessageData::Task(task_msg)) => {
            match task_msg {
                abbot::bus::TaskMsg::Request { task_id, .. } => {
                    tracing::debug!(task_id = %task_id, reply_to = ?msg.reply_to, "task requested");
                    return MessageEvent::TaskRequested {
                        task_id: task_id.clone(),
                        reply_to: msg.reply_to,
                    };
                }
                abbot::bus::TaskMsg::Result { task_id, ok, .. } => {
                    tracing::debug!(task_id = %task_id, ok = %ok, "task completed");
                    return MessageEvent::TaskCompleted {
                        task_id: task_id.clone(),
                    };
                }
                _ => {}
            }
        }

        (MessageOp::Chat, _) => {
            if msg.origin == Origin::Human && !msg.scope.is_head_mail() {
                return MessageEvent::UserMessage { msg_id: msg.id };
            }
        }

        _ => {}
    }
    MessageEvent::None
}

async fn run_memory(cli: Cli, action: MemoryAction) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::sandbox_recall_db;

    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let recall_db_path = sandbox_recall_db(&cli.sandbox)
        .ok_or_else(|| "could not determine recall database path for sandbox")?;
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

fn run_sandbox(cli: Cli, action: SandboxAction) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{data_dir, sandbox_dir, sandbox_workspace, sandbox_db, sandbox_recall_db, sandbox_env, create_sandbox_env, create_sandbox_mind_metadata, create_sandbox_config};

    let data_dir = data_dir().ok_or_else(|| "could not determine data directory")?;

    // Ensure data dir exists
    if !data_dir.exists() {
        std::fs::create_dir_all(&data_dir)?;
    }

    match action {
        SandboxAction::Create { name } => {
            // Validate name
            if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains('.') {
                eprintln!("error: sandbox name cannot be empty or contain path separators or dots");
                std::process::exit(1);
            }

            let workspace = sandbox_workspace(&name).unwrap();
            if workspace.exists() {
                eprintln!("error: sandbox '{}' already exists", name);
                std::process::exit(1);
            }

            std::fs::create_dir_all(&workspace)?;
            create_sandbox_env(&name)?;
            create_sandbox_mind_metadata(&name)?;
            create_sandbox_config(&name)?;

            println!("created sandbox '{}'", name);
            println!("  workspace: {}", workspace.display());
            println!("  env: {}", sandbox_env(&name).unwrap().display());
        }

        SandboxAction::Clone { url, name } => {
            // Derive sandbox name from URL if not provided
            let sandbox_name = name.unwrap_or_else(|| {
                // Extract repo name from URL (e.g., "https://github.com/user/repo.git" -> "repo")
                url.trim_end_matches('/')
                    .trim_end_matches(".git")
                    .rsplit('/')
                    .next()
                    .unwrap_or("repo")
                    .to_string()
            });

            // Validate name
            if sandbox_name.is_empty() || sandbox_name.contains('/') || sandbox_name.contains('\\') || sandbox_name.contains('.') {
                eprintln!("error: derived sandbox name '{}' is invalid, use --name to specify", sandbox_name);
                std::process::exit(1);
            }

            let workspace = sandbox_workspace(&sandbox_name).unwrap();
            if workspace.exists() {
                eprintln!("error: sandbox '{}' already exists", sandbox_name);
                std::process::exit(1);
            }

            println!("cloning {} into sandbox '{}'...", url, sandbox_name);

            let status = std::process::Command::new("git")
                .arg("clone")
                .arg(&url)
                .arg(&workspace)
                .status()?;

            if !status.success() {
                eprintln!("error: git clone failed");
                std::process::exit(1);
            }

            create_sandbox_env(&sandbox_name)?;
            create_sandbox_mind_metadata(&sandbox_name)?;
            create_sandbox_config(&sandbox_name)?;

            println!("created sandbox '{}'", sandbox_name);
            println!("  workspace: {}", workspace.display());
            println!("  env: {}", sandbox_env(&sandbox_name).unwrap().display());
        }

        SandboxAction::List => {
            println!("sandboxes in {}:", data_dir.display());
            println!();

            let mut found = false;
            for entry in std::fs::read_dir(&data_dir)? {
                let entry = entry?;
                let path = entry.path();

                // Sandbox is a directory containing root/ subdir or store.sqlite
                if path.is_dir() {
                    found = true;
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    let db_path = sandbox_db(&name).unwrap();
                    let has_db = db_path.exists();
                    let workspace = sandbox_workspace(&name).unwrap();

                    // Count mounts (symlinks inside root/)
                    let mount_count = std::fs::read_dir(&workspace)
                        .map(|entries| entries.filter_map(|e| e.ok()).filter(|e| e.path().is_symlink()).count())
                        .unwrap_or(0);

                    println!("  {} (db: {}, mounts: {})", name, if has_db { "yes" } else { "no" }, mount_count);
                }
            }

            if !found {
                println!("  (no sandboxes)");
            }
        }

        SandboxAction::Status { name } => {
            let sandbox_name = name.unwrap_or(cli.sandbox);
            let workspace = sandbox_workspace(&sandbox_name).unwrap();
            let db = sandbox_db(&sandbox_name).unwrap();
            let recall_db = sandbox_recall_db(&sandbox_name).unwrap();

            if !workspace.exists() && !db.exists() {
                eprintln!("error: sandbox '{}' does not exist", sandbox_name);
                std::process::exit(1);
            }

            println!("sandbox: {}", sandbox_name);
            println!();

            // Workspace info
            println!("workspace: {}", workspace.display());
            if workspace.exists() {
                // Check if it's a git repo
                let git_dir = workspace.join(".git");
                if git_dir.exists() {
                    println!("  type: git repository");

                    // Get current branch
                    if let Ok(output) = std::process::Command::new("git")
                        .arg("-C")
                        .arg(&workspace)
                        .arg("branch")
                        .arg("--show-current")
                        .output()
                    {
                        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !branch.is_empty() {
                            println!("  branch: {}", branch);
                        }
                    }

                    // Get remote URL
                    if let Ok(output) = std::process::Command::new("git")
                        .arg("-C")
                        .arg(&workspace)
                        .arg("remote")
                        .arg("get-url")
                        .arg("origin")
                        .output()
                    {
                        let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !remote.is_empty() {
                            println!("  remote: {}", remote);
                        }
                    }
                } else {
                    println!("  type: directory");
                }

                // Count files (excluding .git)
                let file_count = walkdir::WalkDir::new(&workspace)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_file())
                    .filter(|e| !e.path().to_string_lossy().contains("/.git/"))
                    .count();
                println!("  files: {}", file_count);
            } else {
                println!("  (not created)");
            }
            println!();

            // Mounts
            println!("mounts:");
            if workspace.exists() {
                let mut mount_count = 0;
                for entry in std::fs::read_dir(&workspace)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_symlink() {
                        mount_count += 1;
                        let target = std::fs::read_link(&path)?;
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        let valid = target.exists();
                        println!("  {} -> {} {}", name, target.display(), if valid { "" } else { "(broken)" });
                    }
                }
                if mount_count == 0 {
                    println!("  (none)");
                }
            } else {
                println!("  (none)");
            }
            println!();

            // Database info
            println!("database: {}", db.display());
            if db.exists() {
                let size = std::fs::metadata(&db)?.len();
                println!("  size: {} KB", size / 1024);
            } else {
                println!("  (not created)");
            }
            println!();

            // Recall database info
            println!("recall db: {}", recall_db.display());
            if recall_db.exists() {
                let size = std::fs::metadata(&recall_db)?.len();
                println!("  size: {} KB", size / 1024);
            } else {
                println!("  (not created)");
            }
        }

        SandboxAction::Delete { name } => {
            if name == "default" {
                eprintln!("error: cannot delete the default sandbox");
                std::process::exit(1);
            }

            let sandbox = sandbox_dir(&name).unwrap();

            if !sandbox.exists() {
                eprintln!("error: sandbox '{}' does not exist", name);
                std::process::exit(1);
            }

            // Delete entire sandbox directory (includes root/, store.sqlite, recall.sqlite)
            std::fs::remove_dir_all(&sandbox)?;
            println!("sandbox '{}' deleted", name);
        }

        SandboxAction::Reset { name } => {
            let workspace = sandbox_workspace(&name).unwrap();
            let db = sandbox_db(&name).unwrap();
            let recall_db = sandbox_recall_db(&name).unwrap();

            if !workspace.exists() {
                eprintln!("error: sandbox '{}' does not exist", name);
                std::process::exit(1);
            }

            // Check if this is a git repo and get remote URL
            let git_remote = std::process::Command::new("git")
                .arg("config")
                .arg("--get")
                .arg("remote.origin.url")
                .current_dir(&workspace)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty());

            // Collect mounts (symlinks) to preserve
            let mounts: Vec<_> = std::fs::read_dir(&workspace)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_symlink())
                .map(|e| {
                    let path = e.path();
                    let name = path.file_name().unwrap().to_string_lossy().to_string();
                    let target = std::fs::read_link(&path).unwrap();
                    (name, target)
                })
                .collect();

            // Delete workspace
            std::fs::remove_dir_all(&workspace)?;

            // Reclone or recreate empty
            if let Some(url) = &git_remote {
                println!("recloning {}...", url);
                let status = std::process::Command::new("git")
                    .arg("clone")
                    .arg(url)
                    .arg(&workspace)
                    .status()?;

                if !status.success() {
                    eprintln!("error: git clone failed");
                    std::process::exit(1);
                }
            } else {
                std::fs::create_dir_all(&workspace)?;
            }

            // Restore mounts
            for (name, target) in &mounts {
                let link_path = workspace.join(name);
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, &link_path)?;
                #[cfg(windows)]
                std::os::windows::fs::symlink_dir(target, &link_path)?;
            }

            // Delete databases
            if db.exists() {
                std::fs::remove_file(&db)?;
            }
            if recall_db.exists() {
                std::fs::remove_file(&recall_db)?;
            }

            // Recreate metadata files
            create_sandbox_env(&name)?;
            create_sandbox_mind_metadata(&name)?;
            create_sandbox_config(&name)?;

            println!("sandbox '{}' reset", name);
            if git_remote.is_some() {
                println!("  recloned from git");
            }
            println!("  preserved {} mount(s)", mounts.len());
        }
    }

    Ok(())
}

fn run_mount(cli: Cli, action: MountAction) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::sandbox_workspace;

    let workspace = sandbox_workspace(&cli.sandbox)
        .ok_or_else(|| "could not determine workspace path for sandbox")?;

    // Ensure workspace exists
    if !workspace.exists() {
        std::fs::create_dir_all(&workspace)?;
    }

    match action {
        MountAction::Add { name, path } => {
            // Validate name (no path separators, not empty)
            if name.is_empty() || name.contains('/') || name.contains('\\') {
                eprintln!("error: mount name cannot be empty or contain path separators");
                std::process::exit(1);
            }

            // Resolve external path to absolute
            let external = if path.is_absolute() {
                path.clone()
            } else {
                std::env::current_dir()?.join(&path)
            };

            // Verify external path exists and is a directory
            if !external.exists() {
                eprintln!("error: path does not exist: {}", external.display());
                std::process::exit(1);
            }
            if !external.is_dir() {
                eprintln!("error: path is not a directory: {}", external.display());
                std::process::exit(1);
            }

            let link_path = workspace.join(&name);

            // Check if mount already exists
            if link_path.exists() || link_path.is_symlink() {
                eprintln!("error: mount '{}' already exists", name);
                std::process::exit(1);
            }

            // Create symlink
            #[cfg(unix)]
            std::os::unix::fs::symlink(&external, &link_path)?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(&external, &link_path)?;

            println!("mounted '{}' -> {}", name, external.display());
        }

        MountAction::Remove { name } => {
            let link_path = workspace.join(&name);

            if !link_path.is_symlink() {
                eprintln!("error: '{}' is not a mount (symlink)", name);
                std::process::exit(1);
            }

            std::fs::remove_file(&link_path)?;
            println!("unmounted '{}'", name);
        }

        MountAction::List => {
            println!("mounts in sandbox '{}':", cli.sandbox);
            println!("  workspace: {}", workspace.display());
            println!();

            let mut found = false;
            for entry in std::fs::read_dir(&workspace)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_symlink() {
                    found = true;
                    let target = std::fs::read_link(&path)?;
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    println!("  {} -> {}", name, target.display());
                }
            }

            if !found {
                println!("  (no mounts)");
            }
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

            println!("Running: ANTHROPIC_BASE_URL={} claude {}", BASE_URL, args.join(" "));

            let status = cmd.status()?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
    }

    Ok(())
}

fn run_export(cli: Cli, name: Option<String>, output: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{sandbox_db, sandbox_workspace};
    use std::io::Write;

    let sandbox_name = name.unwrap_or(cli.sandbox);
    let db_path = sandbox_db(&sandbox_name)
        .ok_or_else(|| "could not determine database path for sandbox")?;
    let workspace = sandbox_workspace(&sandbox_name)
        .ok_or_else(|| "could not determine workspace path for sandbox")?;

    if !db_path.exists() {
        eprintln!("error: sandbox '{}' has no database", sandbox_name);
        std::process::exit(1);
    }

    let store = Store::open(&db_path)?;
    let messages = store.all_messages()?;

    if messages.is_empty() {
        eprintln!("error: sandbox '{}' has no messages", sandbox_name);
        std::process::exit(1);
    }

    // Get first message timestamp for header
    let first_ts = messages.first().map(|m| m.timestamp).unwrap();
    let started = chrono::DateTime::<chrono::Utc>::from(first_ts)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

    // Build output
    let mut out = String::new();

    // Header
    out.push_str(&format!("📋 Sandbox: {}\n", sandbox_name));
    out.push_str(&format!("📋 Workspace: {}\n", workspace.display()));
    out.push_str(&format!("📋 Started: {}\n", started));
    out.push_str(&format!("📋 Messages: {}\n", messages.len()));
    out.push('\n');

    // LTM (Long-Term Memory)
    let ltm = store.get_head_ltm("Monk").unwrap_or_default();
    if !ltm.is_empty() {
        out.push_str("## Long-Term Memory\n\n");
        out.push_str(&ltm);
        out.push_str("\n\n");
    }

    // Wants pool
    if let Ok(wants) = store.list_wants(100) {
        if !wants.is_empty() {
            out.push_str("## Wants Pool\n\n");
            for want in &wants {
                out.push_str(&format!("- [{}] {}\n", want.priority, want.want));
                if !want.context.is_empty() {
                    out.push_str(&format!("  Context: {}\n", want.context));
                }
            }
            out.push('\n');
        }
    }

    out.push_str("## Messages\n\n");

    // Format each message
    for msg in &messages {
        let line = format_message(msg);
        if let Some(line) = line {
            out.push_str(&line);
            out.push('\n');
        }
    }

    // Write output
    if let Some(path) = output {
        let mut file = std::fs::File::create(&path)?;
        file.write_all(out.as_bytes())?;
        eprintln!("exported to {}", path.display());
    } else {
        print!("{}", out);
    }

    Ok(())
}

fn run_plugin(cli: Cli, action: PluginAction) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::sandbox_dir;
    use abbot::runtime::app_config::sandbox_workspace;
    use abbot::runtime::{atomic_write_file_0600, PluginManager};
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
            let enabled = section.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            out.insert(id.to_string(), enabled);
        }

        out
    }

    fn write_plugin_config(path: &std::path::Path, enabled_ids: &HashSet<String>) -> Result<(), Box<dyn std::error::Error>> {
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

    let sandbox = cli.sandbox;
    let Some(dir) = sandbox_dir(&sandbox) else {
        return Err("could not determine sandbox dir".into());
    };
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("plugins.toml");

    let enabled_map = read_plugin_config(&path);
    let mut enabled: HashSet<String> = enabled_map
        .iter()
        .filter_map(|(k, v)| if *v { Some(k.clone()) } else { None })
        .collect();

    match action {
        PluginAction::Enable { name } => {
            enabled.insert(name.clone());
            write_plugin_config(&path, &enabled)?;
            println!("enabled plugin '{}' for sandbox '{}'", name, sandbox);
            println!("reboot required to apply");
        }
        PluginAction::Disable { name } => {
            enabled.remove(&name);
            write_plugin_config(&path, &enabled)?;
            println!("disabled plugin '{}' for sandbox '{}'", name, sandbox);
            println!("reboot required to apply");
        }
        PluginAction::List => {
            let Some(workspace_root) = sandbox_workspace(&sandbox) else {
                return Err("could not determine sandbox workspace".into());
            };

            let mgr = PluginManager::load_for_workspace_root(&workspace_root);
            let catalog = mgr.catalog();

            println!("plugins for sandbox '{}':", sandbox);
            if catalog.is_empty() && enabled.is_empty() {
                println!("(no built-in plugins available)");
                return Ok(());
            }

            for p in &catalog {
                let status = if enabled.contains(&p.id) { "ON " } else { "OFF" };
                let head = format!("head:{}{}", if p.head_expose { "+" } else { "-" }, if p.head_exec { "+" } else { "-" });
                let hand = format!("hand:{}{}", if p.hand_expose { "+" } else { "-" }, if p.hand_exec { "+" } else { "-" });
                println!("{} {:<12} tool={:<12} {} {}  {}", status, p.id, p.tool_name, head, hand, p.description);
            }

            let known_ids: HashSet<String> = catalog.iter().map(|p| p.id.clone()).collect();
            let mut unknown_enabled: Vec<String> = enabled
                .iter()
                .filter(|id| !known_ids.contains(*id))
                .cloned()
                .collect();
            unknown_enabled.sort();

            for id in unknown_enabled {
                println!("ON  {:<12} tool=<unknown>             (unknown plugin id)", id);
            }

            println!("\nrole flags: head=expose/exec, hand=expose/exec (+ = true, - = false)");
        }
    }

    Ok(())
}

fn format_message(msg: &Message) -> Option<String> {
    use abbot::bus::{MessageOp, MessageData, Origin, TaskMsg, NeedMsg, WantMsg};

    let icon = match msg.origin {
        Origin::Human => "👤",
        Origin::Head => "🤖",
        Origin::Hand => "🔧",
        Origin::System => "📋",
    };

    match (&msg.op, &msg.data) {
        // Chat messages - the main content
        (MessageOp::Chat, MessageData::Text(text)) => {
            Some(format!("{} {}", icon, text))
        }

        // Task lifecycle
        (MessageOp::Task, MessageData::Task(task_msg)) => {
            match task_msg {
                TaskMsg::Request { goal, .. } => {
                    Some(format!("📋 Task: {}", goal))
                }
                TaskMsg::Assigned { task_id, hand_id, .. } => {
                    Some(format!("📋 Assigned: {} -> {}", &task_id[..8.min(task_id.len())], hand_id))
                }
                TaskMsg::ToolCall { tool, args, .. } => {
                    let preview: String = args.to_string().chars().take(160).collect();
                    Some(format!("🔧 Call {} {}", tool, preview))
                }
                TaskMsg::ToolDone { tool, ok, duration_ms, error_code, .. } => {
                    if *ok {
                        Some(format!("🔧 Done {} ok ({}ms)", tool, duration_ms))
                    } else if let Some(code) = error_code {
                        Some(format!("🔧 Done {} error={} ({}ms)", tool, code, duration_ms))
                    } else {
                        Some(format!("🔧 Done {} failed ({}ms)", tool, duration_ms))
                    }
                }
                TaskMsg::Echo { tool, content, .. } => {
                    let preview: String = content.chars().take(200).collect();
                    Some(format!("✅ {}: {}", tool, preview.replace('\n', " ")))
                }
                TaskMsg::Result { ok, summary, .. } => {
                    let status = if *ok { "✅" } else { "❌" };
                    Some(format!("{} Result: {}", status, summary))
                }
                TaskMsg::Progress { note, .. } => {
                    Some(format!("📋 Progress: {}", note))
                }
            }
        }

        // Need lifecycle
        (MessageOp::Need, MessageData::Need(need_msg)) => {
            match need_msg {
                NeedMsg::Request { need, priority, .. } => {
                    Some(format!("📋 Need [{:?}]: {}", priority, need))
                }
                NeedMsg::Dispatch { head_id, .. } => {
                    Some(format!("📋 Dispatched to {}", head_id))
                }
                NeedMsg::Acknowledged { head_id, .. } => {
                    Some(format!("📋 Acknowledged by {}", head_id))
                }
                NeedMsg::Fulfilled { summary, .. } => {
                    Some(format!("✅ Fulfilled: {}", summary))
                }
                NeedMsg::Expired { reason, .. } => {
                    Some(format!("⏰ Expired: {}", reason))
                }
            }
        }

        // Want lifecycle
        (MessageOp::Want, MessageData::Want(want_msg)) => {
            match want_msg {
                WantMsg::Added { want, priority, .. } => {
                    Some(format!("Want [{}]: {}", priority, want))
                }
                WantMsg::Removed { want_id, reason } => {
                    Some(format!(
                        "Want {} removed: {}",
                        &want_id[..8.min(want_id.len())],
                        reason
                    ))
                }
                WantMsg::Promoted { want_id, to_priority, need_id } => {
                    if let Some(need_id) = need_id {
                        Some(format!(
                            "Want {} promoted -> {} (need={})",
                            &want_id[..8.min(want_id.len())],
                            to_priority,
                            &need_id[..8.min(need_id.len())]
                        ))
                    } else {
                        Some(format!(
                            "Want {} promoted -> {}",
                            &want_id[..8.min(want_id.len())],
                            to_priority
                        ))
                    }
                }
            }
        }

        // Errors
        (MessageOp::Error, MessageData::Error { message, .. }) => {
            Some(format!("❌ Error: {}", message))
        }

        // Skip internal/system messages
        (MessageOp::Ping, _) => None,
        (MessageOp::Sleep, _) => None,
        (MessageOp::Wake, _) => None,
        (MessageOp::Done, _) => None,
        (MessageOp::Idle, _) => None,
        (MessageOp::Ok, _) => None,
        (MessageOp::Progress, _) => None,
        (MessageOp::Event, _) => None,
        (MessageOp::Item, _) => None,
        (MessageOp::Data, _) => None,

        _ => None,
    }
}
