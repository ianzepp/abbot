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
    AppConfig, ExecService, ExecServiceConfig, GoalService, HandService, HeadService, HeartService,
    RuntimeBus,
};
use abbot::tools::{
    BashTool, CdTool, DiffTool, Dispatcher, EditTool, FindTool, PatchTool, ReadTool, WriteTool,
};

const DEFAULT_DB: &str = "abbot.db";
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

    /// Path to config file
    #[arg(long, env = "ABBOT_CONFIG", default_value = "config.toml")]
    config: PathBuf,
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
    run(cli).await
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv_override();
    tracing_subscriber::fmt::init();
    AppConfig::init(&cli.config);
    tracing::info!(config = %cli.config.display(), "loaded config");

    let cwd = std::env::current_dir()?;
    tracing::info!(cwd = %cwd.display(), "starting in directory");

    let store = Arc::new(Store::open(&cli.db)?);
    tracing::info!(db = %cli.db.display(), "database opened");

    let hub = Arc::new(RwLock::new(abbot::bus::Hub::new()));
    let bus = RuntimeBus::new(hub.clone(), store.clone());

    let head_scope = Scope::main();
    let head_mail_scope = Scope::head_mail(DEFAULT_HEAD_ID);
    let ping_scope = Scope::from(DEFAULT_PING_SCOPE);

    bus.create_scope(head_scope.clone()).await;
    bus.create_scope(head_mail_scope.clone()).await;
    bus.create_scope(ping_scope.clone()).await;

    let dispatcher = default_dispatcher();

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
        default_dispatcher(),
    ))
    .start();

    Arc::new(HeadService::new(
        bus.clone(),
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![head_scope.clone(), head_mail_scope.clone()],
    ))
    .start();

    Arc::new(HeartService::new(
        bus.clone(),
        store.clone(),
        DEFAULT_HEAD_ID,
        vec![head_scope.clone(), head_mail_scope.clone()],
    ))
    .start();

    tracing::info!(
        tick_s = TICK_SECONDS,
        default_sleep_s = DEFAULT_SLEEP_SECONDS,
        debounce_s = WAKE_DEBOUNCE_SECONDS,
        "starting heartbeat loop"
    );

    let mut heads: HashMap<String, HeadState> = HashMap::new();
    heads.insert(DEFAULT_HEAD_ID.to_string(), HeadState::new());

    let mut rx = hub.read().await.subscribe_all();
    let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECONDS));
    let mut tick: u64 = 0;

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
                handle_message(&msg, &mut heads, tick);
            }

            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutdown requested");
                break;
            }
        }
    }

    Ok(())
}

fn handle_message(msg: &Message, heads: &mut HashMap<String, HeadState>, current_tick: u64) {
    match (&msg.op, &msg.data) {
        (MessageOp::Sleep, MessageData::Sleep { seconds }) => {
            if msg.origin == Origin::Head {
                if let Some(state) = heads.get_mut(&msg.sender) {
                    tracing::info!(head = %msg.sender, seconds, "head sleeping");
                    state.schedule_sleep(current_tick, *seconds);
                }
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
