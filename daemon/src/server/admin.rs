// Admin API routes for configuration management.
//
// Provides localhost-only endpoints for reading and updating abbot.toml,
// and querying the logs database.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::sync::RwLock;

use crate::kernel::{FrameSelectArgs, build_frame_select_sql, execute_frame_select};
use crate::runtime::AppConfig;

#[derive(Clone)]
pub struct AdminState {
    config_path: PathBuf,
    frames_db_path: Option<PathBuf>,
    config: Arc<RwLock<AppConfig>>,
}

impl AdminState {
    pub fn new(config_path: PathBuf, frames_db_path: Option<PathBuf>) -> Self {
        let config = AppConfig::load(&config_path);
        Self {
            config_path,
            frames_db_path,
            config: Arc::new(RwLock::new(config)),
        }
    }
}

fn require_localhost(peer_addr: SocketAddr) -> Result<(), StatusCode> {
    if peer_addr.ip().is_loopback() {
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

fn validate_workspace_rel_path(path: &str) -> Result<PathBuf, StatusCode> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Ok(PathBuf::new());
    }

    let p = std::path::Path::new(trimmed);
    for c in p.components() {
        use std::path::Component;
        match c {
            Component::CurDir => {}
            Component::Normal(_) => {}
            Component::ParentDir => return Err(StatusCode::BAD_REQUEST),
            Component::RootDir | Component::Prefix(_) => return Err(StatusCode::BAD_REQUEST),
        }
    }

    Ok(p.to_path_buf())
}

async fn resolve_workspace_path(
    _state: &AdminState,
    rel: &PathBuf,
) -> Result<(String, PathBuf, PathBuf), Response> {
    let Some(ws) = crate::runtime::app_config::config_dir() else {
        return Err(admin_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "could not determine data directory (~/.abbot/)",
        ));
    };

    let workspace_cfg = ws.to_string_lossy().to_string();

    let root = match ws.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return Err(admin_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("data directory path invalid: {e}"),
            ));
        }
    };

    let joined = root.join(rel);
    let joined = match joined.canonicalize() {
        Ok(p) => p,
        Err(_) => joined,
    };

    if !joined.starts_with(&root) {
        return Err(admin_error(
            StatusCode::FORBIDDEN,
            "path outside data directory",
        ));
    }

    Ok((workspace_cfg, root, joined))
}

fn is_sqlite3_db_header(buf: &[u8]) -> bool {
    buf.len() >= 16 && &buf[..16] == b"SQLite format 3\0"
}

fn quote_sqlite_ident(name: &str) -> String {
    let escaped = name.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

async fn sqlite_db_summary(path: &std::path::Path) -> Result<String, String> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .map_err(|e| format!("db open failed: {e}"))?;

    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    let freelist: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);

    let rows = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| format!("query failed: {e}"))?;

    let table_names: Vec<String> = rows.iter().map(|r| r.get::<String, _>(0)).collect();

    let mut lines = Vec::new();
    lines.push("SQLite database".to_string());
    if page_size > 0 && page_count > 0 {
        lines.push(format!(
            "pages: {} (size {} bytes) freelist {}",
            page_count, page_size, freelist
        ));
    }
    lines.push(format!("tables: {}", table_names.len()));
    lines.push(String::new());

    let mut total_rows: i64 = 0;
    for name in table_names.iter().take(200) {
        let ident = quote_sqlite_ident(name);
        let sql = format!("SELECT COUNT(*) FROM {ident}");
        let count: i64 = sqlx::query_scalar(&sql)
            .fetch_one(&pool)
            .await
            .unwrap_or(-1);
        if count >= 0 {
            total_rows += count;
            lines.push(format!("- {name}: {count} rows"));
        } else {
            lines.push(format!("- {name}: (count unavailable)"));
        }
    }

    if table_names.len() > 200 {
        lines.push(String::new());
        lines.push(format!(
            "(showing first 200 tables; {} more)",
            table_names.len() - 200
        ));
    }

    if !table_names.is_empty() {
        lines.push(String::new());
        lines.push(format!("total rows (sum of counts): {total_rows}"));
    }

    pool.close().await;
    Ok(lines.join("\n"))
}

#[derive(Debug, Default, Deserialize)]
pub struct FsListQuery {
    pub path: Option<String>,
}

/// GET /admin/fs/list - List directory entries under the configured workspace.
pub async fn get_fs_list(
    State(state): State<AdminState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Query(query): Query<FsListQuery>,
) -> Response {
    if let Err(status) = require_localhost(peer_addr) {
        return admin_error(status, "admin API requires localhost access");
    }

    let rel_str = query.path.unwrap_or_default();
    let rel = match validate_workspace_rel_path(&rel_str) {
        Ok(p) => p,
        Err(_) => return admin_error(StatusCode::BAD_REQUEST, "invalid path"),
    };

    let (workspace_cfg, _root, abs) = match resolve_workspace_path(&state, &rel).await {
        Ok(v) => v,
        Err(r) => return r,
    };

    let meta = match tokio::fs::metadata(&abs).await {
        Ok(m) => m,
        Err(e) => return admin_error(StatusCode::NOT_FOUND, format!("not found: {e}")),
    };
    if !meta.is_dir() {
        return admin_error(StatusCode::BAD_REQUEST, "path is not a directory");
    }

    let mut rd = match tokio::fs::read_dir(&abs).await {
        Ok(r) => r,
        Err(e) => {
            return admin_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("read_dir failed: {e}"),
            );
        }
    };

    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut count = 0usize;
    while let Ok(Some(entry)) = rd.next_entry().await {
        count += 1;
        if count > 5000 {
            break;
        }

        let file_name = entry.file_name().to_string_lossy().to_string();
        let entry_abs = entry.path();
        let entry_meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = entry_meta.is_dir();
        let size = if entry_meta.is_file() {
            entry_meta.len()
        } else {
            0
        };

        let rel_child = if rel_str.trim().is_empty() {
            file_name.clone()
        } else {
            format!("{}/{}", rel_str.trim_end_matches('/'), file_name)
        };

        let modified_ms = entry_meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);

        let _ = entry_abs;

        items.push(serde_json::json!({
            "name": file_name,
            "path": rel_child,
            "is_dir": is_dir,
            "size": size,
            "modified_ms": modified_ms,
        }));
    }

    items.sort_by(|a, b| {
        let ad = a.get("is_dir").and_then(|v| v.as_bool()).unwrap_or(false);
        let bd = b.get("is_dir").and_then(|v| v.as_bool()).unwrap_or(false);
        match (ad, bd) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                let an = a
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let bn = b
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                an.cmp(&bn)
            }
        }
    });

    Json(serde_json::json!({
        "workspace": workspace_cfg,
        "path": rel_str,
        "count": items.len(),
        "items": items,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct FsReadQuery {
    pub path: String,
    pub max_bytes: Option<usize>,
}

use crate::runtime::provider_cache::load_provider_cache;

#[derive(Debug, Default, Deserialize)]
pub struct ProviderModelsQuery {
    pub provider: Option<String>,
    pub q: Option<String>,
    pub limit: Option<usize>,
}

/// GET /admin/providers/models - Read cached provider model list(s).
pub async fn get_provider_models(
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Query(query): Query<ProviderModelsQuery>,
) -> Response {
    if let Err(status) = require_localhost(peer_addr) {
        return admin_error(status, "admin API requires localhost access");
    }

    let provider_filter = query
        .provider
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let q = query.q.unwrap_or_default();
    let q = q.trim().to_ascii_lowercase();
    let limit = query.limit.unwrap_or(500).clamp(1, 5000);

    let mut providers = Vec::new();
    if let Some(p) = provider_filter {
        providers.push(p.to_string());
    } else {
        providers.extend([
            "openrouter".to_string(),
            "openai".to_string(),
            "anthropic".to_string(),
            "ollama".to_string(),
        ]);
    }

    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut total = 0usize;

    for provider in providers {
        let Some(cache) = load_provider_cache(&provider) else {
            continue;
        };

        for m in cache.models {
            if total >= limit {
                break;
            }

            if !q.is_empty() {
                let id = m.id.to_ascii_lowercase();
                let name = m.name.as_deref().unwrap_or("").to_ascii_lowercase();
                if !id.contains(&q) && !name.contains(&q) {
                    continue;
                }
            }

            items.push(serde_json::json!({
                "provider": cache.provider,
                "fetched_at": cache.fetched_at,
                "id": m.id,
                "name": m.name,
                "context_window": m.context_window,
                "input_cost": m.input_cost,
                "output_cost": m.output_cost,
            }));
            total += 1;
        }
    }

    Json(serde_json::json!({
        "count": items.len(),
        "items": items,
        "limit": limit,
    }))
    .into_response()
}

/// GET /admin/fs/read - Read a file under the configured workspace.
pub async fn get_fs_read(
    State(state): State<AdminState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Query(query): Query<FsReadQuery>,
) -> Response {
    if let Err(status) = require_localhost(peer_addr) {
        return admin_error(status, "admin API requires localhost access");
    }

    let rel = match validate_workspace_rel_path(&query.path) {
        Ok(p) => p,
        Err(_) => return admin_error(StatusCode::BAD_REQUEST, "invalid path"),
    };

    let (_workspace_cfg, _root, abs) = match resolve_workspace_path(&state, &rel).await {
        Ok(v) => v,
        Err(r) => return r,
    };

    let meta = match tokio::fs::metadata(&abs).await {
        Ok(m) => m,
        Err(e) => return admin_error(StatusCode::NOT_FOUND, format!("not found: {e}")),
    };
    if !meta.is_file() {
        return admin_error(StatusCode::BAD_REQUEST, "path is not a file");
    }

    let size_bytes = meta.len();
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);

    let mut header = [0u8; 16];
    if let Ok(mut f) = tokio::fs::File::open(&abs).await {
        let n = f.read(&mut header).await.unwrap_or(0);
        let _ = f.seek(std::io::SeekFrom::Start(0)).await;
        if n == 16 && is_sqlite3_db_header(&header) {
            let summary = sqlite_db_summary(&abs)
                .await
                .unwrap_or_else(|_| "SQLite database (summary unavailable)".to_string());

            let mut content = Vec::new();
            content.push(format!("path: {}", query.path));
            content.push(format!("size_bytes: {}", size_bytes));
            if let Some(ms) = modified_ms {
                content.push(format!("modified_ms: {}", ms));
            }
            content.push(String::new());
            content.push(summary);

            return Json(serde_json::json!({
                "path": query.path,
                "truncated": false,
                "binary": false,
                "content": content.join("\n"),
            }))
            .into_response();
        }
    }

    let max = query.max_bytes.unwrap_or(64 * 1024).clamp(1, 512 * 1024);
    let mut f = match tokio::fs::File::open(&abs).await {
        Ok(f) => f,
        Err(e) => {
            return admin_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("open failed: {e}"),
            );
        }
    };

    let mut buf: Vec<u8> = Vec::with_capacity(max.min(64 * 1024) + 1);
    let mut limited = (&mut f).take((max + 1) as u64);
    if let Err(e) = limited.read_to_end(&mut buf).await {
        return admin_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("read failed: {e}"),
        );
    }

    let truncated = buf.len() > max;
    if truncated {
        buf.truncate(max);
    }

    let has_nul = buf.contains(&0);
    let utf8_ok = std::str::from_utf8(&buf).is_ok();
    let binary = has_nul || !utf8_ok;

    let content = if binary {
        let mut s = Vec::new();
        s.push(format!("path: {}", query.path));
        s.push(format!("size_bytes: {}", size_bytes));
        if let Some(ms) = modified_ms {
            s.push(format!("modified_ms: {}", ms));
        }
        s.push(String::new());
        s.push("(binary file; preview disabled)".to_string());
        s.join("\n")
    } else {
        String::from_utf8_lossy(&buf).to_string()
    };

    Json(serde_json::json!({
        "path": query.path,
        "truncated": truncated,
        "binary": binary,
        "content": content,
    }))
    .into_response()
}

/// GET /admin/config - Full config as JSON
pub async fn get_config(
    State(state): State<AdminState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
) -> Response {
    if let Err(status) = require_localhost(peer_addr) {
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
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Path(section): Path<String>,
) -> Response {
    if let Err(status) = require_localhost(peer_addr) {
        return admin_error(status, "admin API requires localhost access");
    }

    let config = state.config.read().await;
    match config.section_json(&section) {
        Some(json) => Json(json).into_response(),
        None => admin_error(
            StatusCode::NOT_FOUND,
            format!("unknown section: {}", section),
        ),
    }
}

/// PUT /admin/config - Update full config
pub async fn put_config(
    State(state): State<AdminState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Json(update): Json<AppConfig>,
) -> Response {
    if let Err(status) = require_localhost(peer_addr) {
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
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Path(section): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    if let Err(status) = require_localhost(peer_addr) {
        return admin_error(status, "admin API requires localhost access");
    }

    let mut config = state.config.write().await;

    let result = match section.as_str() {
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
        "harness" => serde_json::from_value(value)
            .map(|v| config.harness = v)
            .map_err(|e| e.to_string()),
        "vfs" => serde_json::from_value(value)
            .map(|v| config.vfs = v)
            .map_err(|e| e.to_string()),
        _ => {
            return admin_error(
                StatusCode::NOT_FOUND,
                format!("unknown section: {}", section),
            );
        }
    };

    if let Err(e) = result {
        return admin_error(
            StatusCode::BAD_REQUEST,
            format!("invalid section data: {}", e),
        );
    }

    if let Err(e) = config.save(&state.config_path) {
        return admin_error(StatusCode::INTERNAL_SERVER_ERROR, e);
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}

/// Query parameters for /admin/scopes
#[derive(Debug, Default, Deserialize)]
pub struct ScopesQuery {
    pub limit: Option<u64>,
}

/// GET /admin/scopes - List distinct scopes from the frames database
pub async fn get_scopes(
    State(state): State<AdminState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Query(query): Query<ScopesQuery>,
) -> Response {
    if let Err(status) = require_localhost(peer_addr) {
        return admin_error(status, "admin API requires localhost access");
    }

    let Some(frames_db_path) = &state.frames_db_path else {
        return admin_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "frames database not configured",
        );
    };

    if !frames_db_path.exists() {
        return admin_error(StatusCode::SERVICE_UNAVAILABLE, "frames database not found");
    }

    let opts = SqliteConnectOptions::new()
        .filename(frames_db_path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = match sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            return admin_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db open failed: {e}"),
            );
        }
    };

    let limit = query.limit.unwrap_or(50).clamp(1, 500) as i64;

    let sql = "SELECT scope, MAX(seq) AS last_seq, COUNT(*) AS frame_count \
               FROM frames \
               WHERE scope IS NOT NULL AND scope != '' \
               GROUP BY scope \
               ORDER BY last_seq DESC \
               LIMIT ?";

    let rows = match sqlx::query(sql).bind(limit).fetch_all(&pool).await {
        Ok(r) => r,
        Err(e) => {
            return admin_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("query failed: {e}"),
            );
        }
    };

    let mut items: Vec<serde_json::Value> = Vec::new();
    for row in &rows {
        let scope: String = row.get(0);
        let last_seq: i64 = row.get(1);
        let frame_count: i64 = row.get(2);
        items.push(serde_json::json!({
            "scope": scope,
            "last_seq": last_seq,
            "frame_count": frame_count,
        }));
    }

    pool.close().await;

    Json(serde_json::json!({
        "count": items.len(),
        "items": items,
    }))
    .into_response()
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
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Query(query): Query<LogsQuery>,
) -> Response {
    if let Err(status) = require_localhost(peer_addr) {
        return admin_error(status, "admin API requires localhost access");
    }

    let Some(frames_db_path) = &state.frames_db_path else {
        return admin_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "frames database not configured",
        );
    };

    if !frames_db_path.exists() {
        return admin_error(StatusCode::SERVICE_UNAVAILABLE, "frames database not found");
    }

    let args = FrameSelectArgs {
        query: query.query,
        ops: query
            .ops
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect()),
        kinds: query
            .kinds
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect()),
        actors: query
            .actors
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect()),
        scope: query.scope,
        limit: query.limit,
        order: query.order,
        since_seq: query.since_seq,
        include_frame: Some(true),
        include_json: Some(false),
        ..Default::default()
    };

    let limit = args.limit.unwrap_or(200).clamp(1, 2000) as i64;
    let order = match args
        .order
        .as_deref()
        .unwrap_or("desc")
        .to_lowercase()
        .as_str()
    {
        "asc" => "ASC",
        _ => "DESC",
    };

    let (mut sql, params) = build_frame_select_sql(&args, order, limit);

    // Filter out SIGTICK event entries
    sql = sql.replace(
        " ORDER BY",
        " AND NOT (op = 'Event' AND kind = 'SIGTICK') ORDER BY",
    );

    // Open a temporary SQLx pool for the admin query
    let opts = SqliteConnectOptions::new()
        .filename(frames_db_path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = match sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            return admin_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db open failed: {e}"),
            );
        }
    };

    let rows = match execute_frame_select(&pool, &sql, &params).await {
        Ok(r) => r,
        Err(e) => {
            return admin_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("query failed: {e}"),
            );
        }
    };

    let mut items: Vec<serde_json::Value> = Vec::new();
    for row in &rows {
        let seq: i64 = row.get(0);
        let ts_ms: i64 = row.get(1);
        let op: String = row.get(2);
        let name: Option<String> = row.try_get(3).ok();
        let actor: Option<String> = row.try_get(4).ok();
        let frame_id: String = row.get(5);
        let parent_id: Option<String> = row.try_get(6).ok();
        let scope: Option<String> = row.try_get(7).ok();
        let kind: Option<String> = row.try_get(8).ok();
        let reply_to: Option<String> = row.try_get(9).ok();
        let frame_json: String = row.try_get(10).unwrap_or_else(|_| "{}".to_string());

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
    }))
    .into_response()
}
