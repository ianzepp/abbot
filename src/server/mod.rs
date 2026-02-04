// Server module - OpenAI/Anthropic compatible API endpoints + Web UI.
//
// Exposes abbot as an LLM-compatible API server so users can interact
// using standard OpenAI clients, curl, or any tool that speaks the protocol.
// Also serves the web UI via WebSocket for real-time frame streaming.

mod admin;
mod anthropic;
mod handler;
mod ingress_hub;
mod openai;
mod session_scope;
mod user_prompt;
mod web_chat;
mod websocket;

pub use admin::{AdminState, get_config, get_config_section, put_config, put_config_section};
pub use anthropic::{AnthropicState, messages};
pub use handler::{ChatChunk, ChatHandler, ChatMessage, ChatRequest, Role};
pub use ingress_hub::IngressHub;
pub use openai::{OpenAIState, chat_completions, list_models};
pub use web_chat::{WebChatState, web_chat};
pub use websocket::{WsState, ws_handler};

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::history::Store;
use crate::runtime::default_config_path;

const DEFAULT_ADDR: &str = "127.0.0.1:8080";

pub struct Server {
    store: Arc<Store>,
    head_id: String,
    addr: String,
    web_dist: Option<PathBuf>,
    proxy: bool,
}

impl Server {
    pub fn new(store: Arc<Store>, head_id: impl Into<String>) -> Self {
        Self {
            store,
            head_id: head_id.into(),
            addr: DEFAULT_ADDR.to_string(),
            web_dist: None,
            proxy: false,
        }
    }

    pub fn with_addr(mut self, addr: impl Into<String>) -> Self {
        self.addr = addr.into();
        self
    }

    pub fn with_web_dist(mut self, path: impl Into<PathBuf>) -> Self {
        self.web_dist = Some(path.into());
        self
    }

    pub fn with_proxy(mut self, proxy: bool) -> Self {
        self.proxy = proxy;
        self
    }

    pub async fn start(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let openai_state =
            OpenAIState::new(self.store.clone(), &self.head_id).with_proxy(self.proxy);

        let anthropic_state = AnthropicState::new(self.store.clone(), &self.head_id);
        let ws_state = WsState::new();
        let web_chat_state = WebChatState::new(self.store.clone());

        // OpenAI-compatible routes
        let openai_routes = Router::new()
            .route("/v1/models", get(list_models))
            .route("/v1/chat/completions", post(chat_completions))
            .with_state(openai_state);

        // Build the main app
        let mut app = if self.proxy {
            openai_routes
        } else {
            // Anthropic-compatible routes
            let anthropic_routes = Router::new()
                .route("/v1/messages", post(messages))
                .with_state(anthropic_state);

            // WebSocket route
            let ws_routes = Router::new()
                .route("/ws", get(ws_handler))
                .with_state(ws_state);

            // Web chat route
            let web_chat_routes = Router::new()
                .route("/api/chat", post(web_chat))
                .with_state(web_chat_state);

            // Admin routes (localhost only)
            let admin_routes = if let Some(config_path) = default_config_path() {
                let admin_state = AdminState::new(config_path);
                Router::new()
                    .route("/admin/config", get(get_config).put(put_config))
                    .route("/admin/config/{section}", get(get_config_section).put(put_config_section))
                    .with_state(admin_state)
            } else {
                Router::new()
            };

            openai_routes
                .merge(anthropic_routes)
                .merge(ws_routes)
                .merge(web_chat_routes)
                .merge(admin_routes)
        };

        // Serve static files for web UI if configured
        if let Some(web_dist) = self.web_dist {
            if web_dist.exists() {
                let index_path = web_dist.join("index.html");
                let serve_dir =
                    ServeDir::new(&web_dist).not_found_service(ServeFile::new(&index_path));
                app = app.fallback_service(serve_dir);
                tracing::info!(path = %web_dist.display(), "serving web UI");
            } else {
                tracing::warn!(path = %web_dist.display(), "web UI dist not found");
            }
        }

        // Add CORS for development
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        let app = app.layer(cors);

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
