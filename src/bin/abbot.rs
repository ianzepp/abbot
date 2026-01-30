// Abbot CLI - main entry point for running the server and interacting with it.
//
// High-level command overview:
//
// - `abbot server run`: Start the server (bus + sqlite store + services), plus:
//   - HTTP API server (used by `abbot chat`, `abbot task new`)
//   - Unix socket listener (used by `abbot tui`, `abbot dev tail`)
//
// - `abbot server status|stop`: Basic pidfile-based process management.
//
// - `abbot chat`: Publish a chat message to a scope (`#channel`, `@mail`, `§task/...`) via the HTTP API.
//
// - `abbot task new`: Publish a task request (goal) via the HTTP API; optionally wait for a result by
//   tailing the task scope in sqlite history.
//
// - `abbot tail`: Tail a single scope from sqlite history (polling; useful when no socket available).
//
// - `abbot dev tail`: Developer-focused live tail from the unix socket stream, with filters and optional
//   sqlite history bootstrap.
//
// - `abbot dev thread`: Print a reply thread from sqlite (messages whose `reply_to` matches a message id).
//
// - `abbot tui`: Interactive terminal UI client over the unix socket.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::RwLock;
use uuid::Uuid;

use abbot::api::{ApiClient, ApiRequest, ApiResponse, ApiServer};
use abbot::bus::{MessageData, MessageOp, Origin, Scope, TaskMsg, respond};
use abbot::history::Store;
use abbot::irc::Server as IrcServer;
use abbot::runtime::{
    AppConfig, ExecService, ExecServiceConfig, GoalService, HandService, HeadService, HeartService,
    RuntimeBus,
};
use abbot::socket::SocketListener;
use abbot::tools::{
    BashTool, CdTool, DiffTool, Dispatcher, EditTool, FindTool, PatchTool, ReadTool, WriteTool,
};
use abbot::tui::App as TuiApp;

const DEFAULT_DB: &str = "abbot.db";
const DEFAULT_API_ADDR: &str = "127.0.0.1:7337";
const DEFAULT_SOCKET: &str = "~/.abbot/abbot.sock";
const DEFAULT_HEAD_ID: &str = "Monk";
const DEFAULT_HEAD_SCOPE: &str = "#general";
const DEFAULT_PING_SCOPE: &str = "#ping";

#[derive(Parser, Clone)]
#[command(name = "abbot")]
#[command(about = "Abbot harness: server + CLI", version)]
struct Cli {
    /// Path to sqlite database file
    #[arg(long, env = "ABBOT_DB", default_value = DEFAULT_DB, global = true)]
    db: PathBuf,

    /// API address for server/client (host:port)
    #[arg(long, env = "ABBOT_API_ADDR", default_value = DEFAULT_API_ADDR, global = true)]
    api_addr: String,

    /// Sender identity for CLI-published messages
    #[arg(long, env = "ABBOT_SENDER", default_value = "_user", global = true)]
    sender: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Clone)]
enum Command {
    Server {
        #[command(subcommand)]
        cmd: ServerCmd,
    },
    /// Publish a chat message to a scope (#channel, @mailbox, §task/<id>)
    Chat {
        /// Scope string (e.g. #general, @abbot, §task/t-1)
        scope: String,
        /// Message content
        #[arg(trailing_var_arg = true)]
        content: Vec<String>,
    },
    Task {
        #[command(subcommand)]
        cmd: TaskCmd,
    },
    /// Tail a scope from sqlite history
    Tail {
        scope: String,
        #[arg(short, long, default_value = "50")]
        limit: usize,
        #[arg(short, long)]
        follow: bool,
        #[arg(long)]
        op: Option<String>,
        #[arg(long, default_value = "500")]
        poll_ms: u64,
    },
    /// Developer utilities for debugging a running server
    Dev {
        #[command(subcommand)]
        cmd: DevCmd,
    },
    /// Interactive TUI chat client
    Tui {
        /// Scope to join
        #[arg(default_value = "#general")]
        scope: String,
        /// Unix socket path
        #[arg(long, env = "ABBOT_SOCKET", default_value = DEFAULT_SOCKET)]
        socket: String,
    },
}

#[derive(Subcommand, Clone)]
enum ServerCmd {
    Run {
        /// Path to config file (e.g., config.openai.toml, config.ollama.toml)
        #[arg(long, env = "ABBOT_CONFIG", default_value = "config.toml")]
        config: PathBuf,

        /// Unix socket path for TUI/client connections
        #[arg(long, env = "ABBOT_SOCKET", default_value = DEFAULT_SOCKET)]
        socket: String,

        /// Optional IRC port for humans (starts IRC server if set)
        #[arg(long)]
        irc_port: Option<u16>,

        /// Heartbeat interval seconds
        #[arg(long, default_value = "60")]
        heartbeat_s: u64,

        /// PID file path (written on start)
        #[arg(long, env = "ABBOT_PID_FILE", default_value = "abbot.pid")]
        pid_file: PathBuf,
    },
    Status {
        /// PID file path
        #[arg(long, env = "ABBOT_PID_FILE", default_value = "abbot.pid")]
        pid_file: PathBuf,
    },
    Stop {
        /// PID file path
        #[arg(long, env = "ABBOT_PID_FILE", default_value = "abbot.pid")]
        pid_file: PathBuf,
    },
}

#[derive(Subcommand, Clone)]
enum TaskCmd {
    New {
        /// Task goal
        #[arg(required = true, trailing_var_arg = true)]
        goal: Vec<String>,

        /// Optional task id (defaults to t-<8hex>)
        #[arg(long)]
        id: Option<String>,

        /// Optional additional input/constraints
        #[arg(long, default_value = "")]
        input: String,

        /// Head id (default Monk)
        #[arg(long, default_value = DEFAULT_HEAD_ID)]
        head: String,

        /// Wait for result (tails task scope)
        #[arg(long)]
        wait: bool,
    },
}

#[derive(Subcommand, Clone)]
enum DevCmd {
    /// Stream live messages from the server unix socket (with optional history from sqlite)
    Tail {
        /// Unix socket path (requires server run with socket listener)
        #[arg(long, env = "ABBOT_SOCKET", default_value = DEFAULT_SOCKET)]
        socket: String,

        /// Include recent history from sqlite before streaming
        #[arg(long, default_value = "100")]
        history: usize,

        /// Scopes to include (e.g. #general @Monk §task/t-1). If omitted, streams all scopes.
        #[arg(trailing_var_arg = true)]
        scopes: Vec<String>,

        /// Filter by op (e.g. Chat, Task, Event)
        #[arg(long)]
        op: Option<String>,

        /// Filter by origin (head, hand, human, system)
        #[arg(long)]
        origin: Option<String>,

        /// Filter by sender (exact match)
        #[arg(long)]
        sender: Option<String>,

        /// Filter by reply_to message id (UUID)
        #[arg(long)]
        reply_to: Option<String>,

        /// Filter task messages by task_id (prefix match)
        #[arg(long)]
        task: Option<String>,

        /// Only show messages whose rendered form contains this substring
        #[arg(long)]
        contains: Option<String>,

        /// Print raw JSONL (wire format) instead of pretty printing
        #[arg(long)]
        json: bool,
    },

    /// Show a reply thread (messages whose reply_to == id), from sqlite
    Thread {
        /// Root message id (UUID)
        id: String,

        /// Include the root message itself (if present in sqlite)
        #[arg(long)]
        include_root: bool,

        /// Follow new replies by polling sqlite
        #[arg(long)]
        follow: bool,

        #[arg(long, default_value = "500")]
        poll_ms: u64,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Server { cmd } => match cmd {
            ServerCmd::Run {
                config,
                socket,
                irc_port,
                heartbeat_s,
                pid_file,
            } => {
                server_run(
                    cli.db,
                    cli.api_addr,
                    config,
                    socket,
                    irc_port,
                    heartbeat_s,
                    pid_file,
                )
                .await?
            }
            ServerCmd::Status { pid_file } => server_status(pid_file)?,
            ServerCmd::Stop { pid_file } => server_stop(pid_file)?,
        },
        Command::Chat { scope, content } => {
            let (scope, content) = parse_scope_and_content(scope, content);
            let client = ApiClient::new(cli.api_addr);
            let req = ApiRequest::PublishChat {
                scope,
                sender: cli.sender,
                origin: Origin::Human,
                content,
            };
            match client.request(&req).await? {
                ApiResponse::Ok => {}
                ApiResponse::Error { message } => return Err(message.into()),
                other => return Err(format!("unexpected response: {:?}", other).into()),
            }
        }
        Command::Task { cmd } => match cmd {
            TaskCmd::New {
                goal,
                id,
                input,
                head,
                wait,
            } => {
                let task_id = id.unwrap_or_else(|| format!("t-{}", random_hex8()));
                let scope = format!("§task/{}", task_id);
                let client = ApiClient::new(cli.api_addr.clone());

                let req = ApiRequest::PublishTaskRequest {
                    task_id: task_id.clone(),
                    scope: scope.clone(),
                    sender: cli.sender.clone(),
                    origin: Origin::Human,
                    head_id: head,
                    goal: goal.join(" "),
                    input,
                };
                match client.request(&req).await? {
                    ApiResponse::Ok => {}
                    ApiResponse::Error { message } => return Err(message.into()),
                    other => return Err(format!("unexpected response: {:?}", other).into()),
                }

                println!("{}", scope);
                if wait {
                    tail_task_until_result(&cli.db, &scope, &task_id, 250).await?;
                }
            }
        },
        Command::Tail {
            scope,
            limit,
            follow,
            op,
            poll_ms,
        } => {
            tail_scope(&cli.db, &scope, limit, follow, op, poll_ms).await?;
        }
        Command::Dev { cmd } => match cmd {
            DevCmd::Tail {
                socket,
                history,
                scopes,
                op,
                origin,
                sender,
                reply_to,
                task,
                contains,
                json,
            } => {
                dev_tail(
                    &cli.db,
                    socket,
                    history,
                    scopes,
                    DevTailFilter {
                        op,
                        origin,
                        sender,
                        reply_to,
                        task,
                        contains,
                    },
                    json,
                )
                .await?;
            }
            DevCmd::Thread {
                id,
                include_root,
                follow,
                poll_ms,
            } => {
                dev_thread(&cli.db, &id, include_root, follow, poll_ms).await?;
            }
        },
        Command::Tui { scope, socket } => {
            run_tui(socket, scope)?;
        }
    }

    Ok(())
}

fn parse_scope_and_content(scope: String, content: Vec<String>) -> (String, String) {
    (scope, content.join(" "))
}

#[derive(Clone, Debug, Default)]
struct DevTailFilter {
    op: Option<String>,
    origin: Option<String>,
    sender: Option<String>,
    reply_to: Option<String>,
    task: Option<String>,
    contains: Option<String>,
}

async fn server_run(
    db: PathBuf,
    api_addr: String,
    config: PathBuf,
    socket: String,
    irc_port: Option<u16>,
    heartbeat_s: u64,
    pid_file: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv_override();
    tracing_subscriber::fmt::init();
    AppConfig::init(&config);
    tracing::info!(config = %config.display(), "loaded config");

    write_pid(&pid_file)?;

    let store = std::sync::Arc::new(Store::open(&db)?);
    tracing::info!(db = %db.display(), "database opened");

    let hub = std::sync::Arc::new(RwLock::new(abbot::bus::Hub::new()));
    let bus = RuntimeBus::new(hub.clone(), store.clone());

    bus.create_scope(Scope::from(DEFAULT_HEAD_SCOPE)).await;
    bus.create_scope(Scope::from(DEFAULT_PING_SCOPE)).await;

    ApiServer::new(bus.clone(), api_addr).start();

    let socket_path = expand_tilde(&socket);
    let socket_listener =
        std::sync::Arc::new(SocketListener::new(bus.clone(), socket_path.clone()));
    tokio::spawn(async move {
        if let Err(e) = socket_listener.start().await {
            tracing::error!(error = %e, "socket listener failed");
        }
    });

    // Heartbeat
    let bus_heartbeat = bus.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(heartbeat_s));
        let mut tick: u64 = 0;
        loop {
            interval.tick().await;
            tick += 1;
            tracing::info!(tick, "ping");
            bus_heartbeat
                .publish(
                    respond::ping("_heartbeat", DEFAULT_PING_SCOPE, tick)
                        .with_origin(Origin::System),
                )
                .await;
        }
    });

    // Tools
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(BashTool));
    dispatcher.register(Box::new(CdTool));
    dispatcher.register(Box::new(ReadTool));
    dispatcher.register(Box::new(WriteTool));
    dispatcher.register(Box::new(EditTool));
    dispatcher.register(Box::new(FindTool));
    dispatcher.register(Box::new(DiffTool));
    dispatcher.register(Box::new(PatchTool));

    std::sync::Arc::new(ExecService::new(
        bus.clone(),
        dispatcher,
        ExecServiceConfig::default(),
    ))
    .start();
    std::sync::Arc::new(GoalService::new(bus.clone())).start();
    std::sync::Arc::new(HandService::new(
        bus.clone(),
        store.clone(),
        default_dispatcher(),
    ))
    .start();
    std::sync::Arc::new(HeadService::new(
        bus.clone(),
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![Scope::from(DEFAULT_HEAD_SCOPE)],
    ))
    .start();
    std::sync::Arc::new(HeartService::new(
        bus.clone(),
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![Scope::from(DEFAULT_HEAD_SCOPE)],
    ))
    .start();

    if let Some(port) = irc_port {
        let server = IrcServer::new(bus.clone(), port);
        tracing::info!(port, "starting IRC server");
        let mut task = tokio::spawn(async move { server.run().await });
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutdown requested");
                task.abort();
            }
            _ = &mut task => {}
        }
    } else {
        tracing::info!("server running (no IRC)");
        tokio::signal::ctrl_c().await?;
        tracing::info!("shutdown requested");
    }

    remove_pid(&pid_file);
    Ok(())
}

fn server_status(pid_file: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let Some(pid) = read_pid(&pid_file)? else {
        println!("stopped");
        return Ok(());
    };
    if process_alive(pid) {
        println!("running pid={}", pid);
    } else {
        println!("stale pidfile pid={}", pid);
    }
    Ok(())
}

fn server_stop(pid_file: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let Some(pid) = read_pid(&pid_file)? else {
        println!("stopped");
        return Ok(());
    };
    if !process_alive(pid) {
        println!("not running (stale pidfile pid={})", pid);
        remove_pid(&pid_file);
        return Ok(());
    }

    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args([pid.to_string()])
            .status()
            .ok();
    }
    #[cfg(not(unix))]
    {
        return Err("stop not supported on this platform".into());
    }

    println!("stopped pid={}", pid);
    remove_pid(&pid_file);
    Ok(())
}

fn write_pid(pid_file: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    if pid_file.exists() {
        if let Some(pid) = read_pid(pid_file)? {
            if process_alive(pid) {
                return Err(format!(
                    "server already running pid={} (pidfile {})",
                    pid,
                    pid_file.display()
                )
                .into());
            }
        }
    }
    std::fs::write(pid_file, std::process::id().to_string())?;
    Ok(())
}

fn read_pid(pid_file: &PathBuf) -> Result<Option<u32>, Box<dyn std::error::Error>> {
    if !pid_file.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(pid_file)?;
    Ok(s.trim().parse::<u32>().ok())
}

fn remove_pid(pid_file: &PathBuf) {
    let _ = std::fs::remove_file(pid_file);
}

fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}{}", home.to_string_lossy(), &path[1..]);
        }
    }
    path.to_string()
}

fn run_tui(socket: String, scope: String) -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = expand_tilde(&socket);
    let app = TuiApp::new(socket_path, scope);
    let terminal = ratatui::init();
    let result = app.run(terminal);
    ratatui::restore();
    result.map_err(|e| e.into())
}

fn message_matches_filter(
    msg: &abbot::bus::Message,
    scopes: &[String],
    filter: &DevTailFilter,
) -> bool {
    if !scopes.is_empty() {
        let scope_str = msg.scope.to_string();
        if !scopes.iter().any(|s| s == &scope_str) {
            return false;
        }
    }

    if let Some(op) = &filter.op {
        if format!("{:?}", msg.op) != *op {
            return false;
        }
    }

    if let Some(origin) = &filter.origin {
        if msg.origin.as_str() != origin {
            return false;
        }
    }

    if let Some(sender) = &filter.sender {
        if &msg.sender != sender {
            return false;
        }
    }

    if let Some(reply_to) = &filter.reply_to {
        let Ok(id) = Uuid::parse_str(reply_to) else {
            return false;
        };
        if msg.reply_to != Some(id) {
            return false;
        }
    }

    if let Some(task_prefix) = &filter.task {
        let matches_task = match &msg.data {
            MessageData::Task(TaskMsg::Request { task_id, .. })
            | MessageData::Task(TaskMsg::Assigned { task_id, .. })
            | MessageData::Task(TaskMsg::Echo { task_id, .. })
            | MessageData::Task(TaskMsg::Progress { task_id, .. })
            | MessageData::Task(TaskMsg::Result { task_id, .. }) => {
                task_id.starts_with(task_prefix)
            }
            _ => false,
        };
        if !matches_task {
            return false;
        }
    }

    if let Some(contains) = &filter.contains {
        if !format!("{:?}", msg.data).contains(contains) {
            return false;
        }
    }

    true
}

async fn dev_tail(
    db: &PathBuf,
    socket: String,
    history: usize,
    scopes: Vec<String>,
    filter: DevTailFilter,
    json_out: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = expand_tilde(&socket);
    let store = Store::open(db)?;

    if history > 0 {
        let scan = (history * 20).clamp(history, 5000);
        let mut msgs = store.recent_any(scan)?;
        msgs.retain(|m| message_matches_filter(m, &scopes, &filter));
        if msgs.len() > history {
            msgs = msgs.split_off(msgs.len() - history);
        }
        for m in msgs {
            print_message(&m);
        }
    }

    let stream = UnixStream::connect(socket_path).await?;
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let wire: abbot::socket::WireMessage = match serde_json::from_str(&line) {
            Ok(w) => w,
            Err(_) => continue,
        };
        let msg: abbot::bus::Message = match wire.clone().try_into() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !message_matches_filter(&msg, &scopes, &filter) {
            continue;
        }
        if json_out {
            println!("{}", line);
        } else {
            print_message(&msg);
        }
    }

    Ok(())
}

async fn dev_thread(
    db: &PathBuf,
    id: &str,
    include_root: bool,
    follow: bool,
    poll_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open(db)?;
    let root_id = Uuid::parse_str(id)?;

    if include_root {
        if let Some(root) = store.get(root_id)? {
            print_message(&root);
        }
    }

    let mut printed: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    loop {
        let msgs = store.get_thread(root_id)?;
        for m in msgs {
            if printed.insert(m.id) {
                print_message(&m);
            }
        }

        if !follow {
            break;
        }

        tokio::time::sleep(Duration::from_millis(poll_ms)).await;
    }

    Ok(())
}

async fn tail_scope(
    db: &PathBuf,
    scope: &str,
    limit: usize,
    follow: bool,
    op: Option<String>,
    poll_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open(db)?;
    let mut last_ts: i64 = 0;

    loop {
        let mut msgs = if let Some(op) = &op {
            store.recent_by_op(scope, op, limit)?
        } else {
            store.recent(scope, limit)?
        };

        msgs.retain(|m| {
            let ts = m
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            ts > last_ts
        });

        for m in &msgs {
            let ts = m
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            last_ts = last_ts.max(ts);
            print_message(m);
        }

        if !follow {
            break;
        }
        tokio::time::sleep(Duration::from_millis(poll_ms)).await;
    }

    Ok(())
}

async fn tail_task_until_result(
    db: &PathBuf,
    scope: &str,
    task_id: &str,
    poll_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open(db)?;
    let mut last_ts: i64 = 0;

    loop {
        let mut msgs = store.recent_by_op(scope, "Task", 200)?;
        msgs.retain(|m| {
            let ts = m
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            ts > last_ts
        });

        let mut saw_result = false;
        for m in &msgs {
            let ts = m
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            last_ts = last_ts.max(ts);
            print_message(m);

            if let (MessageOp::Task, MessageData::Task(TaskMsg::Result { task_id: tid, .. })) =
                (&m.op, &m.data)
            {
                if tid == task_id {
                    saw_result = true;
                }
            }
        }

        if saw_result {
            break;
        }

        tokio::time::sleep(Duration::from_millis(poll_ms)).await;
    }

    Ok(())
}

fn print_message(msg: &abbot::bus::Message) {
    let reply = msg
        .reply_to
        .map(|id| id.to_string().chars().take(8).collect::<String>())
        .unwrap_or_default();
    let reply_prefix = if reply.is_empty() {
        "".to_string()
    } else {
        format!("↩{} ", reply)
    };

    match (&msg.op, &msg.data) {
        (MessageOp::Chat, MessageData::Text(t)) => {
            println!(
                "[{}] {} {}{}: {}",
                msg.scope,
                msg.origin.as_str(),
                reply_prefix,
                msg.sender,
                t.trim_end()
            );
        }
        (
            MessageOp::Task,
            MessageData::Task(TaskMsg::Request {
                task_id,
                head_id,
                goal,
                ..
            }),
        ) => {
            println!(
                "[{}] {}task request id={} head={} goal={}",
                msg.scope, reply_prefix, task_id, head_id, goal
            );
        }
        (
            MessageOp::Task,
            MessageData::Task(TaskMsg::Assigned {
                task_id, hand_id, ..
            }),
        ) => {
            println!(
                "[{}] {}task assigned id={} hand={}",
                msg.scope, reply_prefix, task_id, hand_id
            );
        }
        (MessageOp::Task, MessageData::Task(TaskMsg::Echo { tool, content, .. })) => {
            println!(
                "[{}] {}echo tool={}\n{}\n---",
                msg.scope,
                reply_prefix,
                tool,
                content.trim_end()
            );
        }
        (MessageOp::Task, MessageData::Task(TaskMsg::Progress { note, .. })) => {
            println!("[{}] {}progress {}", msg.scope, reply_prefix, note);
        }
        (MessageOp::Task, MessageData::Task(TaskMsg::Result { ok, summary, .. })) => {
            println!(
                "[{}] {}result ok={}\n{}",
                msg.scope,
                reply_prefix,
                ok,
                summary.trim_end()
            );
        }
        (MessageOp::Event, MessageData::Event { kind, payload }) => {
            println!("[{}] {}event {} {}", msg.scope, reply_prefix, kind, payload);
        }
        _ => {}
    }
}

fn default_dispatcher() -> Dispatcher {
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(BashTool));
    dispatcher.register(Box::new(CdTool));
    dispatcher.register(Box::new(ReadTool));
    dispatcher.register(Box::new(WriteTool));
    dispatcher.register(Box::new(EditTool));
    dispatcher.register(Box::new(FindTool));
    dispatcher.register(Box::new(DiffTool));
    dispatcher.register(Box::new(PatchTool));
    dispatcher
}

fn random_hex8() -> String {
    let mut rng = rand::rng();
    let n = rand::RngCore::next_u32(&mut rng);
    format!("{:08x}", n)
}
