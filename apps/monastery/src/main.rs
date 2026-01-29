mod bus;
mod chat;
mod config;
mod github;
mod irc;
mod tools;
mod llm;
mod history;
mod monk;

// Use library exports for shared types
use abbot::{Message, Store};
use config::Config;

use std::sync::Arc;
use std::time::Duration;
use std::path::PathBuf;
use tokio::sync::RwLock;
use bus::{Hub, respond};
use history::Store;
use irc::Server;
use llm::{LlmClient, resolve_model};
use monk::{Monk, MonkFile, Runner, new_registry};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
const HEARTBEAT_CHANNEL: &str = "#ping";

#[tokio::main]
async fn main() {
    // Parse CLI args
    let cfg = Config::parse();

    // Load .env file, overwriting any existing env vars
    let _ = dotenvy::dotenv_override();

    tracing_subscriber::fmt::init();

    // Initialize global config
    config::init(cfg.clone());

    tracing::info!(monastery = %cfg.monastery.display(), order = %cfg.order_path().display(), "starting monastery");

    // Ensure directories exist
    std::fs::create_dir_all(&cfg.monastery).expect("failed to create monastery directory");
    std::fs::create_dir_all(&cfg.monks_dir()).expect("failed to create monks directory");
    std::fs::create_dir_all(&cfg.hermitage_root()).expect("failed to create hermitage directory");

    // Database
    let store = Arc::new(Store::open(cfg.db_path()).expect("failed to open database"));
    tracing::info!(db = %cfg.db_path().display(), "database opened");

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
    let default_model = resolve_model("large");
    let llm_client = match LlmClient::from_env(default_model) {
        Ok(client) => {
            tracing::info!(model = %default_model, "LLM client initialized");
            Some(client)
        }
        Err(e) => {
            tracing::warn!(error = %e, "LLM client not available - monks will not respond");
            None
        }
    };

    // System prompt (loaded from order/ directory)
    let grammar = cfg.load_grammar();
    let rules = cfg.load_system();
    let system = format!("{}\n\n{}", grammar, rules);

    tracing::info!(order = %cfg.order_path().display(), "order config loaded");

    // Helper to create monk hermitage
    let hermitage_root = cfg.hermitage_root();
    let create_monk_hermitage = |monk_id: &str| -> PathBuf {
        let monk_path = hermitage_root.join(monk_id);
        std::fs::create_dir_all(&monk_path).expect("failed to create monk hermitage");
        monk_path.canonicalize().unwrap_or_else(|_| monk_path.clone())
    };

    // Load monks from files in monastery/monks/
    let monk_files = MonkFile::load_all();

    if monk_files.is_empty() {
        // First run - recruit abbot
        tracing::info!("first run - recruiting abbot");

        match MonkFile::recruit("abbot", "large") {
            Ok(file) => {
                tracing::info!(path = %file.path.display(), "created abbot file");
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to recruit abbot");
            }
        }
    }

    // Reload after potential recruitment
    let monk_files = MonkFile::load_all();
    tracing::info!(count = monk_files.len(), "found monk files");

    // Wake all monks that are marked as "awake" or are the abbot
    for file in monk_files {
        let should_wake = file.meta.status == "awake" || file.meta.name == "abbot";

        if !should_wake {
            tracing::info!(name = %file.meta.name, status = %file.meta.status, "skipping dormant monk");
            continue;
        }

        let monk_id = &file.meta.name;
        let monk_model = resolve_model(&file.meta.model);
        let monk_llm = LlmClient::from_env(monk_model).ok();

        // Build system prompt from soul
        let system = file.system_prompt();

        // Create and configure monk
        let mut monk = Monk::new(monk_id, store.clone(), system);
        monk.set_cwd(create_monk_hermitage(monk_id));
        if let Some(llm) = monk_llm {
            monk.set_llm(llm);
        }
        monk.set_hub(hub.clone());
        monk.set_registry(registry.clone());

        // Create cell channel
        let cell = format!("#cell-{}", monk_id);
        if monk_id != "abbot" {
            hub.write().await.create_channel(&cell);
        }

        // Register and subscribe
        let mut reg = registry.write().await;
        reg.add(monk);

        if monk_id == "abbot" {
            reg.subscribe(monk_id, "#general");
        } else {
            reg.subscribe(monk_id, &cell);
            reg.subscribe("abbot", &cell);
        }
        reg.subscribe(monk_id, HEARTBEAT_CHANNEL);

        // Ensure monk exists in database for state storage
        if let Err(e) = store.create_monk(monk_id, &file.meta.model) {
            tracing::warn!(name = %monk_id, error = %e, "failed to create monk in database");
        }

        tracing::info!(name = %monk_id, model = %monk_model, status = %file.meta.status, "woke monk");
    }

    tracing::info!("monastery ready");

    // Runner (dispatches messages to monks)
    let runner = Runner::new(hub.clone(), registry.clone());
    tokio::spawn(async move {
        runner.run().await;
    });

    // GitHub poller (optional - watches for issue comments)
    if let Ok(repo) = std::env::var("GITHUB_REPO") {
        let poller = github::Poller::new(hub.clone(), store.clone(), repo.clone());
        tokio::spawn(async move {
            poller.run().await;
        });
        tracing::info!(repo = %repo, "GitHub poller started");
    }

    // IRC server for humans
    let server = Server::new(hub.clone(), 6667);
    tracing::info!("starting IRC server on port 6667");

    if let Err(e) = server.run().await {
        tracing::error!(?e, "IRC server error");
    }
}
