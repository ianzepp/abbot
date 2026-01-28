mod bus;
mod chat;
mod agent;
mod irc;
mod tools;
mod llm;
mod history;

use std::sync::Arc;
use tokio::sync::RwLock;
use bus::{Hub, Message, MessageOp};
use agent::{Agent, AgentContext};
use irc::Server;
use tools::{Dispatcher, BashTool, DiffTool, EditTool, FindTool, ReadTool};
use llm::LlmClient;
use history::{Store, HistoryAgent};

const HISTORY_CONTEXT_SIZE: usize = 20;

struct BotAgent {
    dispatcher: Arc<RwLock<Dispatcher>>,
    llm: Option<Arc<LlmClient>>,
    store: Arc<Store>,
}

impl BotAgent {
    fn new(dispatcher: Dispatcher, llm: Option<LlmClient>, store: Arc<Store>) -> Self {
        Self {
            dispatcher: Arc::new(RwLock::new(dispatcher)),
            llm: llm.map(Arc::new),
            store,
        }
    }
}

impl Agent for BotAgent {
    fn name(&self) -> &str {
        "abbot"
    }

    fn channels(&self) -> Vec<&str> {
        vec!["#general"]
    }

    async fn on_message(&self, ctx: &AgentContext, msg: Message) {
        // Only respond to chat messages
        if msg.op != MessageOp::Chat {
            return;
        }

        let content = match msg.text() {
            Some(t) => t,
            None => return,
        };

        let response = if content.starts_with('!') {
            self.dispatcher.read().await.dispatch(content).await
        } else if let Some(llm) = &self.llm {
            let history = self.store.recent(&msg.channel, HISTORY_CONTEXT_SIZE).unwrap_or_default();
            match llm.chat(content, &history).await {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::error!(?e, "LLM error");
                    Some(format!("error: {}", e))
                }
            }
        } else {
            None
        };

        if let Some(response) = response {
            for line in response.lines() {
                if !line.is_empty() {
                    ctx.client.say(&msg.channel, line).await;
                }
            }
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let store = Arc::new(Store::open("abbot.db").expect("failed to open database"));
    tracing::info!("database opened: abbot.db");

    let hub = Arc::new(RwLock::new(Hub::new()));
    hub.write().await.create_channel("#general");

    // History agent - records all messages
    let history_agent = HistoryAgent::new(store.clone());
    let hub_clone = hub.clone();
    tokio::spawn(async move {
        history_agent.run(hub_clone).await;
    });

    // Tools
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(BashTool));
    dispatcher.register(Box::new(DiffTool));
    dispatcher.register(Box::new(EditTool));
    dispatcher.register(Box::new(FindTool));
    dispatcher.register(Box::new(ReadTool));

    // LLM
    let llm = match LlmClient::from_env("anthropic/claude-sonnet-4") {
        Ok(client) => {
            tracing::info!("LLM enabled");
            Some(client)
        }
        Err(e) => {
            tracing::warn!(?e, "LLM disabled");
            None
        }
    };

    // Bot agent - handles commands and LLM
    let bot = BotAgent::new(dispatcher, llm, store);
    let hub_clone = hub.clone();
    tokio::spawn(async move {
        bot.run(hub_clone).await;
    });

    let server = Server::new(hub.clone(), 6667);
    tracing::info!("starting abbot");

    if let Err(e) = server.run().await {
        tracing::error!(?e, "IRC server error");
    }
}
