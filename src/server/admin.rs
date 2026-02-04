// Admin API routes for configuration management.
//
// Provides localhost-only endpoints for reading and updating abbot.toml.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tokio::sync::RwLock;

use crate::runtime::AppConfig;

#[derive(Clone)]
pub struct AdminState {
    config_path: PathBuf,
    config: Arc<RwLock<AppConfig>>,
}

impl AdminState {
    pub fn new(config_path: PathBuf) -> Self {
        let config = AppConfig::load(&config_path);
        Self {
            config_path,
            config: Arc::new(RwLock::new(config)),
        }
    }
}

fn require_localhost(headers: &HeaderMap) -> Result<(), StatusCode> {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if host.starts_with("127.0.0.1")
        || host.starts_with("localhost")
        || host.starts_with("[::1]")
    {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn admin_error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": {
                "message": message.into(),
            }
        })),
    )
        .into_response()
}

/// GET /admin/config - Full config as JSON
pub async fn get_config(
    State(state): State<AdminState>,
    headers: HeaderMap,
) -> Response {
    if let Err(status) = require_localhost(&headers) {
        return admin_error(status, "admin API requires localhost access");
    }

    let config = state.config.read().await;
    match serde_json::to_value(&*config) {
        Ok(json) => Json(json).into_response(),
        Err(e) => admin_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to serialize config: {}", e),
        ),
    }
}

/// GET /admin/config/{section} - Single section as JSON
pub async fn get_config_section(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Path(section): Path<String>,
) -> Response {
    if let Err(status) = require_localhost(&headers) {
        return admin_error(status, "admin API requires localhost access");
    }

    let config = state.config.read().await;
    match config.section_json(&section) {
        Some(json) => Json(json).into_response(),
        None => admin_error(StatusCode::NOT_FOUND, format!("unknown section: {}", section)),
    }
}

/// PUT /admin/config - Update full config
pub async fn put_config(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(update): Json<AppConfig>,
) -> Response {
    if let Err(status) = require_localhost(&headers) {
        return admin_error(status, "admin API requires localhost access");
    }

    if let Err(e) = update.save(&state.config_path) {
        return admin_error(StatusCode::INTERNAL_SERVER_ERROR, e);
    }

    {
        let mut config = state.config.write().await;
        *config = update;
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}

/// PUT /admin/config/{section} - Update single section
pub async fn put_config_section(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Path(section): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    if let Err(status) = require_localhost(&headers) {
        return admin_error(status, "admin API requires localhost access");
    }

    let mut config = state.config.write().await;

    let result = match section.as_str() {
        "workspace" => {
            config.workspace = value.as_str().map(|s| s.to_string());
            Ok(())
        }
        "server" => serde_json::from_value(value)
            .map(|v| config.server = v)
            .map_err(|e| e.to_string()),
        "providers" => serde_json::from_value(value)
            .map(|v| config.providers = v)
            .map_err(|e| e.to_string()),
        "head" => serde_json::from_value(value)
            .map(|v| config.head = v)
            .map_err(|e| e.to_string()),
        "hand" => serde_json::from_value(value)
            .map(|v| config.hand = v)
            .map_err(|e| e.to_string()),
        "mind" => serde_json::from_value(value)
            .map(|v| config.mind = v)
            .map_err(|e| e.to_string()),
        "prompt_cache" => serde_json::from_value(value)
            .map(|v| config.prompt_cache = v)
            .map_err(|e| e.to_string()),
        "pool" => serde_json::from_value(value)
            .map(|v| config.pool = v)
            .map_err(|e| e.to_string()),
        "harness" => serde_json::from_value(value)
            .map(|v| config.harness = v)
            .map_err(|e| e.to_string()),
        "vfs" => serde_json::from_value(value)
            .map(|v| config.vfs = v)
            .map_err(|e| e.to_string()),
        _ => return admin_error(StatusCode::NOT_FOUND, format!("unknown section: {}", section)),
    };

    if let Err(e) = result {
        return admin_error(StatusCode::BAD_REQUEST, format!("invalid section data: {}", e));
    }

    if let Err(e) = config.save(&state.config_path) {
        return admin_error(StatusCode::INTERNAL_SERVER_ERROR, e);
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}
