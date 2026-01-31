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
    AppConfig, GoalService, HandService, HeadService, MindService, NeedService, RuntimeBus,
};
use abbot::server::Server;
use abbot::memory::{ensure_schema as ensure_memory_schema, Indexer, Ollama, Search};

const DEFAULT_SANDBOX: &str = "default";
const DEFAULT_HEAD_ID: &str = "Abbot";
const DEFAULT_PING_SCOPE: &str = "ping";

const TICK_SECONDS: u64 = 60;
const DEFAULT_SLEEP_SECONDS: u64 = 300;

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

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Clone)]
enum Command {
    /// Run the daemon (default)
    Run,
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

    match &cli.command {
        None | Some(Command::Run) => run_daemon(cli).await,
        Some(Command::Memory { action }) => run_memory(cli.clone(), action.clone()).await,
        Some(Command::Opencode { action }) => run_opencode(cli.clone(), action.clone()).await,
        Some(Command::Claude { action }) => run_claude(cli.clone(), action.clone()).await,
        Some(Command::Mount { action }) => run_mount(cli.clone(), action.clone()),
    }
}

async fn run_daemon(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    use abbot::runtime::app_config::{default_config_path, sandbox_workspace, sandbox_db, sandbox_memory_db};

    let _ = dotenvy::dotenv_override();
    tracing_subscriber::fmt::init();

    if let Some(ref path) = cli.config {
        AppConfig::init(path);
        tracing::info!(config = %path.display(), "loaded config");
    } else if let Some(path) = default_config_path() {
        AppConfig::init(&path);
        tracing::info!(config = %path.display(), "loaded config");
    } else {
        AppConfig::init_default();
        tracing::warn!("could not determine config path, using defaults");
    }

    // Resolve sandbox paths
    let workspace_path = sandbox_workspace(&cli.sandbox)
        .ok_or_else(|| "could not determine data directory for sandbox")?;
    let db_path = sandbox_db(&cli.sandbox)
        .ok_or_else(|| "could not determine database path for sandbox")?;
    let memory_db_path = sandbox_memory_db(&cli.sandbox)
        .ok_or_else(|| "could not determine memory database path for sandbox")?;

    // Create sandbox workspace directory if it doesn't exist
    if !workspace_path.exists() {
        std::fs::create_dir_all(&workspace_path)?;
        tracing::info!(path = %workspace_path.display(), "created sandbox workspace");
    }

    tracing::info!(
        sandbox = %cli.sandbox,
        workspace = %workspace_path.display(),
        db = %db_path.display(),
        memory_db = %memory_db_path.display(),
        "sandbox initialized"
    );

    // Set working directory to sandbox workspace
    std::env::set_current_dir(&workspace_path)?;

    let store = Arc::new(Store::open(&db_path)?);
    tracing::info!(db = %db_path.display(), "database opened");

    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let memory_search: Option<Arc<Search>> = match rusqlite::Connection::open(&memory_db_path) {
        Ok(conn) => {
            if let Err(e) = ensure_memory_schema(&conn) {
                tracing::warn!(error = %e, "failed to init memory schema");
                None
            } else {
                tracing::info!(db = %memory_db_path.display(), "memory database opened");
                Some(Arc::new(Search::new(conn, Ollama::local())))
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to open memory database");
            None
        }
    };

    let hub = Arc::new(RwLock::new(abbot::bus::Hub::new()));
    let bus = RuntimeBus::new(hub.clone(), store.clone());

    let head_scope = Scope::main();
    let head_mail_scope = Scope::head_mail(DEFAULT_HEAD_ID);
    let ping_scope = Scope::from(DEFAULT_PING_SCOPE);

    bus.create_scope(head_scope.clone()).await;
    bus.create_scope(head_mail_scope.clone()).await;
    bus.create_scope(ping_scope.clone()).await;

    Arc::new(GoalService::new(bus.clone())).start();
    Arc::new(NeedService::new(bus.clone())).start();

    Arc::new(HandService::new(bus.clone(), store.clone())).start();

    // Start head pool (NeedService will dispatch needs to these)
    for i in 0..3 {
        let head_id = format!("head-{}", i);
        Arc::new(HeadService::new(
            bus.clone(),
            store.clone(),
            &head_id,
            vec![head_scope.clone()],  // scopes for context building
            memory_search.clone(),
        ))
        .start();
    }

    Arc::new(MindService::new(
        bus.clone(),
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![head_scope.clone(), head_mail_scope.clone()],
    ))
    .start();

    Server::new(bus.clone(), store.clone(), DEFAULT_HEAD_ID)
        .with_addr(&cli.addr)
        .spawn();

    let exit = cli.exit;
    let initial_prompt = cli.prompt.clone();

    if let Some(ref prompt) = initial_prompt {
        tracing::info!(prompt = %prompt, "sending initial prompt as need");
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
                Scope::from("@need_service"),
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

    tracing::info!(
        tick_s = TICK_SECONDS,
        default_sleep_s = DEFAULT_SLEEP_SECONDS,
        exit = exit,
        "starting heartbeat loop"
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

                        // Emit Idle when everything is done
                        let total_pending: usize = pending_chains.values().sum();
                        if total_pending == 0 {
                            tracing::debug!("emitting Idle (system fully idle)");
                            bus.publish(
                                respond::idle("_harness", Scope::main())
                                    .with_origin(Origin::System),
                            )
                            .await;

                            if exit && prompt_sent {
                                tracing::info!("exit mode: head finished and no pending tasks, exiting");
                                break;
                            }
                        }
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
            }

            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutdown requested");
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
    use abbot::runtime::app_config::sandbox_memory_db;

    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let memory_db_path = sandbox_memory_db(&cli.sandbox)
        .ok_or_else(|| "could not determine memory database path for sandbox")?;
    let conn = rusqlite::Connection::open(&memory_db_path)?;
    ensure_memory_schema(&conn)?;

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

            println!("Memory index stats:");
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
