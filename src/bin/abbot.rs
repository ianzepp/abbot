use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::sync::RwLock;

use abbot::api::{ApiClient, ApiRequest, ApiResponse, ApiServer};
use abbot::bus::{MessageData, MessageOp, Origin, Scope, TaskMsg, respond};
use abbot::history::Store;
use abbot::irc::Server as IrcServer;
use abbot::runtime::{ExecService, ExecServiceConfig, HandAllocator, HandService, HeadService, RuntimeBus};
use abbot::tools::{BashTool, CdTool, DiffTool, Dispatcher, EditTool, FindTool, PatchTool, ReadTool, WriteTool};

const DEFAULT_DB: &str = "abbot.db";
const DEFAULT_API_ADDR: &str = "127.0.0.1:7337";
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
        scope: Option<String>,
        /// Message content
        #[arg(required = true, trailing_var_arg = true)]
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
}

#[derive(Subcommand, Clone)]
enum ServerCmd {
    Run {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Server { cmd } => match cmd {
            ServerCmd::Run {
                irc_port,
                heartbeat_s,
                pid_file,
            } => server_run(cli.db, cli.api_addr, irc_port, heartbeat_s, pid_file).await?,
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
            TaskCmd::New { goal, id, input, head, wait } => {
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
        Command::Tail { scope, limit, follow, op, poll_ms } => {
            tail_scope(&cli.db, &scope, limit, follow, op, poll_ms).await?;
        }
    }

    Ok(())
}

fn parse_scope_and_content(scope: Option<String>, content: Vec<String>) -> (String, String) {
    let joined = content.join(" ");
    let Some(scope) = scope else {
        return (DEFAULT_HEAD_SCOPE.to_string(), joined);
    };
    if scope.starts_with('#') || scope.starts_with('@') || scope.starts_with('§') {
        (scope, joined)
    } else {
        // Treat the provided "scope" as the first content token.
        (DEFAULT_HEAD_SCOPE.to_string(), format!("{} {}", scope, joined).trim().to_string())
    }
}

async fn server_run(
    db: PathBuf,
    api_addr: String,
    irc_port: Option<u16>,
    heartbeat_s: u64,
    pid_file: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv_override();
    tracing_subscriber::fmt::init();

    write_pid(&pid_file)?;

    let store = std::sync::Arc::new(Store::open(&db)?);
    tracing::info!(db = %db.display(), "database opened");

    let hub = std::sync::Arc::new(RwLock::new(abbot::bus::Hub::new()));
    let bus = RuntimeBus::new(hub.clone(), store.clone());

    bus.create_scope(Scope::from(DEFAULT_HEAD_SCOPE)).await;
    bus.create_scope(Scope::from(DEFAULT_PING_SCOPE)).await;

    ApiServer::new(bus.clone(), api_addr).start();

    // Heartbeat
    let bus_heartbeat = bus.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(heartbeat_s));
        let mut tick: u64 = 0;
        loop {
            interval.tick().await;
            tick += 1;
            bus_heartbeat
                .publish(respond::ping("_heartbeat", DEFAULT_PING_SCOPE, tick).with_origin(Origin::System))
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

    std::sync::Arc::new(ExecService::new(bus.clone(), dispatcher, ExecServiceConfig::default())).start();
    std::sync::Arc::new(HandAllocator::new(bus.clone())).start();
    std::sync::Arc::new(HandService::new(bus.clone(), store.clone(), default_dispatcher())).start();
    std::sync::Arc::new(HeadService::new(bus.clone(), DEFAULT_HEAD_ID, Scope::from(DEFAULT_HEAD_SCOPE))).start();

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
                return Err(format!("server already running pid={} (pidfile {})", pid, pid_file.display()).into());
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
            let ts = m.timestamp.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64).unwrap_or(0);
            ts > last_ts
        });

        for m in &msgs {
            let ts = m.timestamp.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64).unwrap_or(0);
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

            if let (MessageOp::Task, MessageData::Task(TaskMsg::Result { task_id: tid, .. })) = (&m.op, &m.data) {
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
    match (&msg.op, &msg.data) {
        (MessageOp::Chat, MessageData::Text(t)) => {
            println!("[{}] {} {}: {}", msg.scope, msg.origin.as_str(), msg.sender, t.trim_end());
        }
        (MessageOp::Task, MessageData::Task(TaskMsg::Request { task_id, head_id, goal, .. })) => {
            println!("[{}] task request id={} head={} goal={}", msg.scope, task_id, head_id, goal);
        }
        (MessageOp::Task, MessageData::Task(TaskMsg::Assigned { task_id, hand_id, .. })) => {
            println!("[{}] task assigned id={} hand={}", msg.scope, task_id, hand_id);
        }
        (MessageOp::Task, MessageData::Task(TaskMsg::Echo { tool, content, .. })) => {
            println!("[{}] echo tool={}\n{}\n---", msg.scope, tool, content.trim_end());
        }
        (MessageOp::Task, MessageData::Task(TaskMsg::Progress { note, .. })) => {
            println!("[{}] progress {}", msg.scope, note);
        }
        (MessageOp::Task, MessageData::Task(TaskMsg::Result { ok, summary, .. })) => {
            println!("[{}] result ok={}\n{}", msg.scope, ok, summary.trim_end());
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
