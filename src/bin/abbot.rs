// Abbot - Persistent AI background daemon.
//
// Runs a heartbeat loop scoped to the starting directory.
// The message bus is the nervous system for a single collective:
// - 1 Head (decision maker, will scale to multiple later)
// - 1 Heart (reflection, long-term memory)
// - N Hands (task executors)

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use tokio::sync::RwLock;

use abbot::bus::{Origin, Scope, respond};
use abbot::history::Store;
use abbot::runtime::{
    AppConfig, ExecService, ExecServiceConfig, GoalService, HandService, HeadService, HeartService,
    RuntimeBus,
};
use abbot::tools::{
    BashTool, CdTool, DiffTool, Dispatcher, EditTool, FindTool, PatchTool, ReadTool, WriteTool,
};

const DEFAULT_DB: &str = "abbot.db";
const DEFAULT_HEAD_ID: &str = "Monk";
const DEFAULT_HEAD_SCOPE: &str = "#general";
const DEFAULT_PING_SCOPE: &str = "#ping";

#[derive(Parser, Clone)]
#[command(name = "abbot")]
#[command(about = "Abbot: persistent AI background daemon", version)]
struct Cli {
    /// Path to sqlite database file
    #[arg(long, env = "ABBOT_DB", default_value = DEFAULT_DB)]
    db: PathBuf,

    /// Path to config file
    #[arg(long, env = "ABBOT_CONFIG", default_value = "config.toml")]
    config: PathBuf,

    /// Tick interval in seconds (base heartbeat)
    #[arg(long, default_value = "1")]
    tick_s: u64,

    /// Head wake interval in ticks (default: check every 60s)
    #[arg(long, default_value = "60")]
    head_wake: u64,

    /// Heart wake interval in ticks (default: reflect every 3600s / 1hr)
    #[arg(long, default_value = "3600")]
    heart_wake: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    run(cli).await
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv_override();
    tracing_subscriber::fmt::init();
    AppConfig::init(&cli.config);
    tracing::info!(config = %cli.config.display(), "loaded config");

    let cwd = std::env::current_dir()?;
    tracing::info!(cwd = %cwd.display(), "starting in directory");

    let store = std::sync::Arc::new(Store::open(&cli.db)?);
    tracing::info!(db = %cli.db.display(), "database opened");

    let hub = std::sync::Arc::new(RwLock::new(abbot::bus::Hub::new()));
    let bus = RuntimeBus::new(hub.clone(), store.clone());

    bus.create_scope(Scope::from(DEFAULT_HEAD_SCOPE)).await;
    let head_mail_scope = format!("@{}", DEFAULT_HEAD_ID);
    bus.create_scope(Scope::from(head_mail_scope.as_str())).await;
    bus.create_scope(Scope::from(DEFAULT_PING_SCOPE)).await;

    let dispatcher = default_dispatcher();

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
        vec![
            Scope::from(DEFAULT_HEAD_SCOPE),
            Scope::from(head_mail_scope.as_str()),
        ],
    ))
    .start();

    std::sync::Arc::new(HeartService::new(
        bus.clone(),
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![
            Scope::from(DEFAULT_HEAD_SCOPE),
            Scope::from(head_mail_scope.as_str()),
        ],
    ))
    .start();

    tracing::info!(
        tick_s = cli.tick_s,
        head_wake = cli.head_wake,
        heart_wake = cli.heart_wake,
        "starting heartbeat loop"
    );

    let mut interval = tokio::time::interval(Duration::from_secs(cli.tick_s));
    let mut tick: u64 = 0;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                tick += 1;

                if tick % cli.head_wake == 0 {
                    tracing::debug!(tick, "head wake");
                    bus.publish(
                        respond::ping("_heartbeat", DEFAULT_PING_SCOPE, tick)
                            .with_origin(Origin::System),
                    )
                    .await;
                }

                if tick % cli.heart_wake == 0 {
                    tracing::debug!(tick, "heart wake");
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
