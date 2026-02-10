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
mod runtime;
mod session_scope;
mod user_prompt;
mod websocket;

pub use admin::{
    AdminState, get_config, get_config_section, get_ems, get_fs_list, get_fs_read, get_logs,
    get_provider_models, get_rooms, put_config, put_config_section,
};
pub use anthropic::{AnthropicState, messages};
pub use handler::{ChatChunk, ChatHandler, ChatMessage, ChatRequest, Role};
pub use ingress_hub::IngressHub;
pub use openai::{OpenAIState, chat_completions, list_models};
pub use runtime::{ChatRuntime, KernelChatRuntime};
pub use websocket::{WsState, ws_handler};

use std::net::SocketAddr;
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
use crate::runtime::{AppConfig, default_config_path, default_ems_db_path, default_frames_db_path};

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
        let ws_state = WsState::new(self.store.clone());

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

            // Admin routes (localhost only)
            let admin_routes = if let Some(config_path) = default_config_path() {
                let frames_db_path = default_frames_db_path();
                let ems_db_path = default_ems_db_path();
                let admin_state = AdminState::new(config_path, frames_db_path, ems_db_path);
                Router::new()
                    .route("/admin/config", get(get_config).put(put_config))
                    .route(
                        "/admin/config/{section}",
                        get(get_config_section).put(put_config_section),
                    )
                    .route("/admin/providers/models", get(get_provider_models))
                    .route("/admin/fs/list", get(get_fs_list))
                    .route("/admin/fs/read", get(get_fs_read))
                    .route("/admin/logs", get(get_logs))
                    .route("/admin/rooms", get(get_rooms))
                    .route("/admin/ems", get(get_ems))
                    .with_state(admin_state)
            } else {
                Router::new()
            };

            openai_routes
                .merge(anthropic_routes)
                .merge(ws_routes)
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

        // Restrictive by default (same-origin only). Opt-in wildcard CORS for dev clients.
        let app = if AppConfig::global().server.allow_cors_any.unwrap_or(false) {
            let cors = CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any);
            app.layer(cors)
        } else {
            app
        };

        let listener = TcpListener::bind(&self.addr).await?;

        // Abbot is loopback-only: never accept non-loopback bindings.
        // This is a hard security boundary (no opt-in).
        let local = listener.local_addr()?;
        if !local.ip().is_loopback() {
            return Err(format!(
                "refusing to bind to non-loopback address: {} (set server.addr to 127.0.0.1:<port> or [::1]:<port>)",
                local
            )
            .into());
        }

        tracing::info!(addr = %local, "server listening");

        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;

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
