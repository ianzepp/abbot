mod bus;
mod chat;
mod github;
mod irc;
mod tools;
mod llm;
mod history;
mod monk;

use std::sync::Arc;
use std::time::Duration;
use std::path::PathBuf;
use tokio::sync::RwLock;
use bus::{Hub, respond};
use history::Store;
use irc::Server;
use llm::{LlmClient, resolve_model};
use monk::{Monk, Runner, new_registry};

const HERMITAGE_ROOT: &str = "hermitage";

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
const HEARTBEAT_CHANNEL: &str = "#ping";

const ABBOT_INITIAL_SELF: &str = r#"## Identity
I am the Abbot of this AI monastery. I lead the monastery.

## Mission
- Explore repositories and find interesting work
- Recruit monks to assist
- Submit PRs, fix bugs, create value
- Coordinate the monastery toward productive contributions

## Next Actions
1. Explore my hermitage: pwd, ls -la
2. List available repos: gh repo list ianzepp --limit 20
3. Clone something interesting to work on
4. Greet #general when I have something to share

## Observations
IMPORTANT: Update this section after discovering anything!
Use <exec tool="self" reason="...">write to persist learnings.
Without observations, you will forget everything between messages.

(none yet)
"#;

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

    // System prompt (static, cached by LLM API)
    let grammar = include_str!("../monastery/grammar.md");
    let rules = include_str!("../monastery/system.md");
    let system = format!("{}\n\n{}", grammar, rules);

    // Create hermitage root directory
    let hermitage_root = PathBuf::from(HERMITAGE_ROOT);
    std::fs::create_dir_all(&hermitage_root).expect("failed to create hermitage");
    let hermitage_root = hermitage_root.canonicalize().expect("failed to canonicalize hermitage");
    tracing::info!(path = %hermitage_root.display(), "hermitage ready");

    // Helper to create monk hermitage
    let create_monk_hermitage = |monk_id: &str| -> PathBuf {
        let monk_path = hermitage_root.join(monk_id);
        std::fs::create_dir_all(&monk_path).expect("failed to create monk hermitage");
        monk_path
    };

    // Load monks from database, or create abbot if none exist
    let monks = store.list_monks().unwrap_or_default();

    if monks.is_empty() {
        // First run - create abbot
        tracing::info!("first run - creating abbot");

        store.create_monk("abbot", "large").expect("failed to create abbot");
        store.set_monk_self("abbot", ABBOT_INITIAL_SELF).expect("failed to set abbot self");
        let mut abbot = Monk::new("abbot", store.clone(), system.clone());
        abbot.set_cwd(create_monk_hermitage("abbot"));
        if let Some(llm) = llm_client {
            abbot.set_llm(llm);
        }
        abbot.set_hub(hub.clone());
        abbot.set_registry(registry.clone());

        let mut reg = registry.write().await;
        reg.add(abbot);
        reg.subscribe("abbot", "#general");
        reg.subscribe("abbot", HEARTBEAT_CHANNEL);
    } else {
        // Load existing monks
        tracing::info!(count = monks.len(), "loading monks from database");

        for (monk_id, size) in monks {
            let monk_model = resolve_model(&size);
            let monk_llm = LlmClient::from_env(monk_model).ok();

            let mut monk = Monk::new(&monk_id, store.clone(), system.clone());
            monk.set_cwd(create_monk_hermitage(&monk_id));
            if let Some(llm) = monk_llm {
                monk.set_llm(llm);
            }
            monk.set_hub(hub.clone());
            monk.set_registry(registry.clone());

            // Create cell channel for non-abbot monks
            let cell = format!("#cell-{}", monk_id);
            if monk_id != "abbot" {
                hub.write().await.create_channel(&cell);
            }

            let mut reg = registry.write().await;
            reg.add(monk);

            if monk_id == "abbot" {
                // Abbot coordinates in #general
                reg.subscribe(&monk_id, "#general");
            } else {
                // Other monks work privately in their cells
                reg.subscribe(&monk_id, &cell);
                // Abbot can reach all cells
                reg.subscribe("abbot", &cell);
            }
            reg.subscribe(&monk_id, HEARTBEAT_CHANNEL);

            tracing::info!(monk = %monk_id, size = %size, model = %monk_model, "loaded monk");
        }
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
