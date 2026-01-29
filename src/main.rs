mod bus;
mod chat;
mod irc;
mod tools;
mod llm;
mod history;
mod monk;

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use bus::{Hub, respond};
use history::Store;
use irc::Server;
use llm::LlmClient;
use monk::{Monk, Runner, new_registry};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
const HEARTBEAT_CHANNEL: &str = "#ping";
const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4";

#[tokio::main]
async fn main() {
    // Load .env file, overwriting any existing env vars
    let _ = dotenvy::dotenv_override();

    tracing_subscriber::fmt::init();

    // Database
    let store = Arc::new(Store::open("abbot.db").expect("failed to open database"));
    tracing::info!("database opened");

    // Pub/sub hub
    let hub = Arc::new(RwLock::new(Hub::new()));
    hub.write().await.create_channel("#general");
    hub.write().await.create_channel(HEARTBEAT_CHANNEL);

    // Heartbeat task
    let hub_heartbeat = hub.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        let mut tick: u64 = 0;
        loop {
            interval.tick().await;
            tick += 1;
            let ping = respond::ping("_heartbeat", HEARTBEAT_CHANNEL, tick);
            hub_heartbeat.read().await.publish(HEARTBEAT_CHANNEL, ping);
            tracing::trace!(tick, "heartbeat");
        }
    });

    // Monk registry
    let registry = new_registry();

    // LLM client (optional - will work without it but won't process messages)
    let llm_client = match LlmClient::from_env(DEFAULT_MODEL) {
        Ok(client) => {
            let key_len = std::env::var("OPENROUTER_API_KEY").map(|k| k.len()).unwrap_or(0);
            tracing::info!(model = DEFAULT_MODEL, key_len, "LLM client initialized");
            Some(client)
        }
        Err(e) => {
            tracing::warn!(error = %e, "LLM client not available - monks will not respond");
            None
        }
    };

    // System prompt (static, cached by LLM API)
    let grammar = include_str!("../monastery/grammar.md");
    let rules = include_str!("../monastery/system.md");
    let system = format!("{}\n\n{}", grammar, rules);

    // Create abbot (the main monk)
    let mut abbot = Monk::new("abbot", store.clone(), system);
    if let Some(llm) = llm_client {
        abbot.set_llm(llm);
    }
    abbot.set_hub(hub.clone());
    abbot.set_registry(registry.clone());

    // Register abbot and subscribe to channels
    {
        let mut reg = registry.write().await;
        reg.add(abbot);
        reg.subscribe("abbot", "#general");
        reg.subscribe("abbot", HEARTBEAT_CHANNEL);
    }

    tracing::info!("abbot registered");

    // Runner (dispatches messages to monks)
    let runner = Runner::new(hub.clone(), registry.clone());
    tokio::spawn(async move {
        runner.run().await;
    });

    // IRC server for humans
    let server = Server::new(hub.clone(), 6667);
    tracing::info!("starting IRC server on port 6667");

    if let Err(e) = server.run().await {
        tracing::error!(?e, "IRC server error");
    }
}
