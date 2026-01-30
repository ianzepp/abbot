// Abbot - Persistent AI background daemon.
//
// Runs a heartbeat loop scoped to the starting directory.
// The message bus is the nervous system for a single collective:
// - 1 Head (decision maker, will scale to multiple later)
// - 1 Heart (reflection, long-term memory)
// - N Hands (task executors)
//
// Timing model:
// - Tick: 60 seconds (fixed)
// - Default sleep: 300 seconds (5 ticks)
// - Wake debounce: 5 seconds

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use tokio::sync::RwLock;

use abbot::bus::{Message, MessageData, MessageOp, Origin, Scope, respond};
use abbot::history::Store;
use abbot::runtime::{
    AppConfig, ExecService, ExecServiceConfig, GoalService, HandService, HeadService, MindService,
    RuntimeBus,
};
use abbot::server::Server;
use abbot::memory::{ensure_schema as ensure_memory_schema, Indexer, Ollama, Search};
use abbot::tools::{
    BashTool, CdTool, DiffTool, Dispatcher, EditTool, FindTool, PatchTool, ReadTool, RecallTool,
    WriteTool,
};

const DEFAULT_DB: &str = "abbot.db";
const DEFAULT_MEMORY_DB: &str = "memory.db";
const DEFAULT_HEAD_ID: &str = "Monk";
const DEFAULT_HEAD_SCOPE: &str = "main";
const DEFAULT_PING_SCOPE: &str = "ping";

const TICK_SECONDS: u64 = 60;
const DEFAULT_SLEEP_SECONDS: u64 = 300;
const WAKE_DEBOUNCE_SECONDS: u64 = 5;

#[derive(Parser, Clone)]
#[command(name = "abbot")]
#[command(about = "Abbot: persistent AI background daemon", version)]
struct Cli {
    /// Path to sqlite database file
    #[arg(long, env = "ABBOT_DB", default_value = DEFAULT_DB)]
    db: PathBuf,

    /// Path to memory/vector database file
    #[arg(long, env = "ABBOT_MEMORY_DB", default_value = DEFAULT_MEMORY_DB)]
    memory_db: PathBuf,

    /// Path to config file
    #[arg(long, env = "ABBOT_CONFIG", default_value = "config.toml")]
    config: PathBuf,

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

struct HeadState {
    wake_at_tick: u64,
    pending_wake: Option<Instant>,
}

impl HeadState {
    fn new() -> Self {
        Self {
            wake_at_tick: 0,
            pending_wake: None,
        }
    }

    fn schedule_sleep(&mut self, current_tick: u64, seconds: u64) {
        let ticks = (seconds + TICK_SECONDS - 1) / TICK_SECONDS;
        self.wake_at_tick = current_tick + ticks;
        self.pending_wake = None;
    }

    fn should_wake(&self, current_tick: u64) -> bool {
        if let Some(pending) = self.pending_wake {
            if pending.elapsed() >= Duration::from_secs(WAKE_DEBOUNCE_SECONDS) {
                return true;
            }
        }
        current_tick >= self.wake_at_tick
    }

    fn trigger_pending_wake(&mut self) {
        if self.pending_wake.is_none() {
            self.pending_wake = Some(Instant::now());
        }
    }

    fn clear_pending(&mut self) {
        self.pending_wake = None;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match &cli.command {
        None | Some(Command::Run) => run_daemon(cli).await,
        Some(Command::Memory { action }) => run_memory(cli.clone(), action.clone()).await,
    }
}

async fn run_daemon(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv_override();
    tracing_subscriber::fmt::init();
    AppConfig::init(&cli.config);
    tracing::info!(config = %cli.config.display(), "loaded config");

    let cwd = std::env::current_dir()?;
    tracing::info!(cwd = %cwd.display(), "starting in directory");

    let store = Arc::new(Store::open(&cli.db)?);
    tracing::info!(db = %cli.db.display(), "database opened");

    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let memory_search = match rusqlite::Connection::open(&cli.memory_db) {
        Ok(conn) => {
            if let Err(e) = ensure_memory_schema(&conn) {
                tracing::warn!(error = %e, "failed to init memory schema");
                None
            } else {
                tracing::info!(db = %cli.memory_db.display(), "memory database opened");
                Some(Search::new(conn, Ollama::local()))
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

    let dispatcher = make_dispatcher(None);

    Arc::new(ExecService::new(
        bus.clone(),
        dispatcher,
        ExecServiceConfig::default(),
    ))
    .start();

    Arc::new(GoalService::new(bus.clone())).start();

    Arc::new(HandService::new(
        bus.clone(),
        store.clone(),
        make_dispatcher(memory_search),
    ))
    .start();

    Arc::new(HeadService::new(
        bus.clone(),
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![head_scope.clone(), head_mail_scope.clone()],
    ))
    .start();

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
        tracing::info!(prompt = %prompt, "sending initial prompt");
        tokio::time::sleep(Duration::from_millis(100)).await;

        bus.publish(
            respond::chat("user", Scope::head_mail(DEFAULT_HEAD_ID), prompt)
                .with_origin(Origin::Human),
        )
        .await;
    }

    tracing::info!(
        tick_s = TICK_SECONDS,
        default_sleep_s = DEFAULT_SLEEP_SECONDS,
        debounce_s = WAKE_DEBOUNCE_SECONDS,
        exit = exit,
        "starting heartbeat loop"
    );

    let mut heads: HashMap<String, HeadState> = HashMap::new();
    heads.insert(DEFAULT_HEAD_ID.to_string(), HeadState::new());

    let mut rx = hub.read().await.subscribe_all();
    let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECONDS));
    let mut tick: u64 = 0;
    let mut prompt_sent = initial_prompt.is_some();
    let mut pending_tasks: usize = 0;
    let mut head_sleeping = false;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                tick += 1;
                tracing::debug!(tick, "tick");

                for (head_id, state) in heads.iter_mut() {
                    if state.should_wake(tick) {
                        tracing::info!(head = %head_id, tick, "waking head");
                        state.clear_pending();
                        state.schedule_sleep(tick, DEFAULT_SLEEP_SECONDS);

                        bus.publish(
                            respond::wake("_harness", Scope::head_mail(head_id), tick)
                                .with_origin(Origin::System),
                        )
                        .await;
                    }
                }
            }

            msg = rx.recv() => {
                let Ok(msg) = msg else { continue };
                let event = handle_message(&msg, &mut heads, tick);

                match event {
                    MessageEvent::HeadSlept => {
                        head_sleeping = true;
                    }
                    MessageEvent::TaskRequested => {
                        pending_tasks += 1;
                        head_sleeping = false;
                    }
                    MessageEvent::TaskCompleted => {
                        pending_tasks = pending_tasks.saturating_sub(1);
                    }
                    MessageEvent::None => {}
                }

                if exit && prompt_sent && head_sleeping && pending_tasks == 0 {
                    tracing::info!("exit mode: head finished and no pending tasks, exiting");
                    break;
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
    TaskRequested,
    TaskCompleted,
}

fn handle_message(msg: &Message, heads: &mut HashMap<String, HeadState>, current_tick: u64) -> MessageEvent {
    match (&msg.op, &msg.data) {
        (MessageOp::Sleep, MessageData::Sleep { seconds }) => {
            if msg.origin == Origin::Head {
                if let Some(state) = heads.get_mut(&msg.sender) {
                    tracing::info!(head = %msg.sender, seconds, "head sleeping");
                    state.schedule_sleep(current_tick, *seconds);
                    return MessageEvent::HeadSlept;
                }
            }
        }

        (MessageOp::Task, MessageData::Task(task_msg)) => {
            match task_msg {
                abbot::bus::TaskMsg::Request { task_id, .. } => {
                    tracing::debug!(task_id = %task_id, "task requested");
                    return MessageEvent::TaskRequested;
                }
                abbot::bus::TaskMsg::Result { task_id, ok, .. } => {
                    tracing::debug!(task_id = %task_id, ok = %ok, "task completed");
                    return MessageEvent::TaskCompleted;
                }
                _ => {}
            }
        }

        (MessageOp::Chat, _) => {
            if msg.origin == Origin::Human {
                if let Some(head_id) = msg.scope.head_id() {
                    if msg.scope.is_head_mail() {
                        if let Some(state) = heads.get_mut(head_id) {
                            tracing::debug!(head = %head_id, "message received, debouncing wake");
                            state.trigger_pending_wake();
                        }
                    }
                }
            }
        }

        _ => {}
    }
    MessageEvent::None
}

fn make_dispatcher(search: Option<Search>) -> Dispatcher {
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(BashTool));
    dispatcher.register(Box::new(CdTool));
    dispatcher.register(Box::new(ReadTool));
    dispatcher.register(Box::new(WriteTool));
    dispatcher.register(Box::new(EditTool));
    dispatcher.register(Box::new(FindTool));
    dispatcher.register(Box::new(DiffTool));
    dispatcher.register(Box::new(PatchTool));
    if let Some(s) = search {
        dispatcher.register(Box::new(RecallTool::new(s)));
    }
    dispatcher
}

async fn run_memory(cli: Cli, action: MemoryAction) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let conn = rusqlite::Connection::open(&cli.memory_db)?;
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
