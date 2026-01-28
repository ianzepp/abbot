use std::sync::Arc;
use tokio::sync::RwLock;
use rand::Rng;
use super::{Tool, ExecutionContext};
use crate::bus::Hub;
use crate::agent::{SharedRegistry, AgentHandle, Monk};
use crate::llm::LlmClient;

const MONK_MODEL: &str = "anthropic/claude-sonnet-4";

pub struct MonkTool {
    hub: Arc<RwLock<Hub>>,
    registry: SharedRegistry,
    api_key: String,
}

impl MonkTool {
    pub fn new(hub: Arc<RwLock<Hub>>, registry: SharedRegistry, api_key: String) -> Self {
        Self { hub, registry, api_key }
    }

    async fn summon(&self) -> String {
        // Generate hex ID
        let id: String = format!("{:08x}", rand::rng().random::<u32>());
        let channel = format!("#monk-{}", id);

        // Create channel
        self.hub.write().await.create_channel(&channel);

        // Create LLM client for the monk
        let llm = match LlmClient::new(&self.api_key, MONK_MODEL) {
            Ok(c) => c,
            Err(e) => return format!("error: {}", e),
        };

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        // Create and spawn the monk
        let monk = Monk::new(id.clone(), channel.clone(), llm);
        let hub_clone = self.hub.clone();

        tokio::spawn(async move {
            monk.run(hub_clone, shutdown_rx).await;
        });

        // Register the monk
        let handle = AgentHandle::new(id.clone(), channel.clone(), MONK_MODEL.to_string(), shutdown_tx);
        self.registry.lock().await.register(handle);

        tracing::info!(monk_id = id, channel = channel, "summoned monk");

        channel
    }

    async fn dismiss(&self, id: &str) -> String {
        if id.is_empty() {
            return self.list().await;
        }

        let mut registry = self.registry.lock().await;
        match registry.dismiss(id) {
            Some(channel) => {
                tracing::info!(monk_id = id, channel, "dismissed monk");
                format!("dismissed {}", id)
            }
            None => format!("error: monk {} not found", id),
        }
    }

    async fn list(&self) -> String {
        let registry = self.registry.lock().await;
        let monks = registry.list();
        if monks.is_empty() {
            return "no active monks".to_string();
        }
        monks.iter()
            .map(|(id, channel, model)| format!("{} {} ({})", id, channel, model))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait::async_trait]
impl Tool for MonkTool {
    fn name(&self) -> &str {
        "monk"
    }

    fn description(&self) -> &str {
        "Manage monks: monk summon, monk dismiss [id], monk list"
    }

    async fn execute(&self, args: &str, _ctx: &ExecutionContext) -> String {
        let args = args.trim();
        let (cmd, rest) = args.split_once(' ').unwrap_or((args, ""));
        let rest = rest.trim();

        match cmd {
            "summon" => self.summon().await,
            "dismiss" => self.dismiss(rest).await,
            "list" => self.list().await,
            "" => self.list().await,
            _ => "usage: monk summon, monk dismiss [id], monk list".to_string(),
        }
    }
}
