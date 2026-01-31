// Server module - OpenAI/Anthropic compatible API endpoints.
//
// Exposes abbot as an LLM-compatible API server so users can interact
// using standard OpenAI clients, curl, or any tool that speaks the protocol.

mod anthropic;
mod handler;
mod openai;

pub use handler::{ChatChunk, ChatHandler, ChatMessage, ChatRequest, Role};
pub use openai::{OpenAIState, chat_completions, list_models};
pub use anthropic::{AnthropicState, messages};

use std::sync::Arc;

use axum::{Router, routing::{get, post}};
use tokio::net::TcpListener;

use crate::history::Store;
use crate::runtime::RuntimeBus;

const DEFAULT_ADDR: &str = "127.0.0.1:8080";

pub struct Server {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
    addr: String,
}

impl Server {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, head_id: impl Into<String>) -> Self {
        Self {
            bus,
            store,
            head_id: head_id.into(),
            addr: DEFAULT_ADDR.to_string(),
        }
    }

    pub fn with_addr(mut self, addr: impl Into<String>) -> Self {
        self.addr = addr.into();
        self
    }

    pub async fn start(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let openai_state = OpenAIState::new(
            self.bus.clone(),
            self.store.clone(),
            &self.head_id,
        );

        let anthropic_state = AnthropicState::new(
            self.bus.clone(),
            self.store.clone(),
            &self.head_id,
        );

        let openai_routes = Router::new()
            .route("/v1/models", get(list_models))
            .route("/v1/chat/completions", post(chat_completions))
            .with_state(openai_state);

        let anthropic_routes = Router::new()
            .route("/v1/messages", post(messages))
            .with_state(anthropic_state);

        let app = openai_routes.merge(anthropic_routes);

        let listener = TcpListener::bind(&self.addr).await?;
        tracing::info!(addr = %self.addr, "server listening");

        axum::serve(listener, app).await?;

        Ok(())
    }

    pub fn spawn(self) {
        let addr = self.addr.clone();
        tokio::spawn(async move {
            if let Err(e) = self.start().await {
                tracing::error!(error = %e, "server failed");
            }
        });
        tracing::info!(addr = %addr, "server spawned");
    }
}
