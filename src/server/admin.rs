// Admin API routes for configuration management.
//
// Provides localhost-only endpoints for reading and updating abbot.toml,
// and querying the logs database.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params_from_iter, Connection};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::kernel::{build_log_select_sql, LogSelectArgs};
use crate::runtime::AppConfig;

#[derive(Clone)]
pub struct AdminState {
    config_path: PathBuf,
    logs_db_path: Option<PathBuf>,
    config: Arc<RwLock<AppConfig>>,
}

impl AdminState {
    pub fn new(config_path: PathBuf, logs_db_path: Option<PathBuf>) -> Self {
        let config = AppConfig::load(&config_path);
        Self {
            config_path,
            logs_db_path,
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

/// Query parameters for /admin/logs
#[derive(Debug, Default, Deserialize)]
pub struct LogsQuery {
    pub query: Option<String>,
    pub name: Option<String>,
    pub ops: Option<String>,
    pub kinds: Option<String>,
    pub actors: Option<String>,
    pub scope: Option<String>,
    pub limit: Option<u64>,
    pub order: Option<String>,
    pub since_seq: Option<u64>,
}

/// GET /admin/logs - Query log frames
pub async fn get_logs(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Query(query): Query<LogsQuery>,
) -> Response {
    if let Err(status) = require_localhost(&headers) {
        return admin_error(status, "admin API requires localhost access");
    }

    let Some(logs_db_path) = &state.logs_db_path else {
        return admin_error(StatusCode::SERVICE_UNAVAILABLE, "logs database not configured");
    };

    if !logs_db_path.exists() {
        return admin_error(StatusCode::SERVICE_UNAVAILABLE, "logs database not found");
    }

    let args = LogSelectArgs {
        query: query.query,
        ops: query.ops.map(|s| s.split(',').map(|x| x.trim().to_string()).collect()),
        kinds: query.kinds.map(|s| s.split(',').map(|x| x.trim().to_string()).collect()),
        actors: query.actors.map(|s| s.split(',').map(|x| x.trim().to_string()).collect()),
        scope: query.scope,
        limit: query.limit,
        order: query.order,
        since_seq: query.since_seq,
        include_frame: Some(true),
        include_json: Some(false),
        ..Default::default()
    };

    let limit = args.limit.unwrap_or(200).clamp(1, 2000) as i64;
    let order = match args.order.as_deref().unwrap_or("desc").to_lowercase().as_str() {
        "asc" => "ASC",
        _ => "DESC",
    };

    let (mut sql, params) = build_log_select_sql(&args, order, limit);

    // Filter out SIGTICK event entries
    sql = sql.replace(" ORDER BY", " AND NOT (op = 'Event' AND kind = 'SIGTICK') ORDER BY");

    let conn = match Connection::open(logs_db_path) {
        Ok(c) => c,
        Err(e) => return admin_error(StatusCode::INTERNAL_SERVER_ERROR, format!("db open failed: {e}")),
    };

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => return admin_error(StatusCode::INTERNAL_SERVER_ERROR, format!("query failed: {e}")),
    };

    let mut rows = match stmt.query(params_from_iter(params)) {
        Ok(r) => r,
        Err(e) => return admin_error(StatusCode::INTERNAL_SERVER_ERROR, format!("query failed: {e}")),
    };

    let mut items: Vec<serde_json::Value> = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let seq: i64 = row.get(0).unwrap_or(0);
        let ts_ms: i64 = row.get(1).unwrap_or(0);
        let op: String = row.get(2).unwrap_or_default();
        let name: Option<String> = row.get(3).ok();
        let actor: Option<String> = row.get(4).ok();
        let frame_id: String = row.get(5).unwrap_or_default();
        let parent_id: Option<String> = row.get(6).ok();
        let scope: Option<String> = row.get(7).ok();
        let kind: Option<String> = row.get(8).ok();
        let reply_to: Option<String> = row.get(9).ok();
        let frame_json: String = row.get(10).unwrap_or_else(|_| "{}".to_string());

        items.push(serde_json::json!({
            "seq": seq,
            "ts_ms": ts_ms,
            "op": op,
            "name": name,
            "actor": actor,
            "frame_id": frame_id,
            "parent_id": parent_id,
            "scope": scope,
            "kind": kind,
            "reply_to": reply_to,
            "frame": serde_json::from_str::<serde_json::Value>(&frame_json).unwrap_or_default(),
        }));
    }

    Json(serde_json::json!({
        "count": items.len(),
        "items": items,
    })).into_response()
}
