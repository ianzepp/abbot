use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::sync::RwLock;

use abbot::bus::{Origin, respond};
use abbot::bus::Hub;
use abbot::bus::Scope;
use abbot::history::Store;
use abbot::irc::Server;
use abbot::runtime::{ExecService, ExecServiceConfig, HandAllocator, HandService, HeadService, RuntimeBus};
use abbot::tools::{BashTool, CdTool, DiffTool, Dispatcher, EditTool, FindTool, PatchTool, ReadTool, WriteTool};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
const HEARTBEAT_CHANNEL: &str = "#ping";

#[derive(Parser, Clone)]
#[command(name = "abbotd", about = "Abbot daemon - persistent pubsub + tool exec harness")]
struct Config {
    /// Path to sqlite database file
    #[arg(long, env = "ABBOT_DB", default_value = "abbot.db")]
    db: String,

    /// IRC server port (localhost)
    #[arg(long, env = "ABBOT_PORT", default_value = "6667")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::parse();

    let _ = dotenvy::dotenv_override();
    tracing_subscriber::fmt::init();

    let store = Arc::new(Store::open(&cfg.db)?);
    tracing::info!(db = %cfg.db, "database opened");

    let hub = Arc::new(RwLock::new(Hub::new()));
    let bus = RuntimeBus::new(hub.clone(), store.clone());

    bus.create_scope(Scope::from("#general")).await;
    bus.create_scope(Scope::from(HEARTBEAT_CHANNEL)).await;

    // Heartbeat task
    let bus_heartbeat = bus.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        let mut tick: u64 = 0;
        loop {
            interval.tick().await;
            tick += 1;
            bus_heartbeat
                .publish(
                    respond::ping("_heartbeat", HEARTBEAT_CHANNEL, tick)
                        .with_origin(Origin::System),
                )
                .await;
        }
    });

    // Exec service (handles MessageOp::Exec)
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(BashTool));
    dispatcher.register(Box::new(CdTool));
    dispatcher.register(Box::new(ReadTool));
    dispatcher.register(Box::new(WriteTool));
    dispatcher.register(Box::new(EditTool));
    dispatcher.register(Box::new(FindTool));
    dispatcher.register(Box::new(DiffTool));
    dispatcher.register(Box::new(PatchTool));

    Arc::new(ExecService::new(bus.clone(), dispatcher, ExecServiceConfig::default())).start();
    Arc::new(HandAllocator::new(bus.clone())).start();
    Arc::new(HandService::new(bus.clone(), store.clone(), default_dispatcher())).start();
    Arc::new(HeadService::new(bus.clone(), "Monk", Scope::from("#general"))).start();

    // IRC server (adapter)
    let server = Server::new(bus.clone(), cfg.port);
    tracing::info!(port = cfg.port, "starting IRC server");

    let mut server_task = tokio::spawn(async move { server.run().await });
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown requested");
            server_task.abort();
            Ok(())
        }
        res = &mut server_task => {
            match res {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e.into()),
                Err(e) => Err(e.into()),
            }
        }
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
