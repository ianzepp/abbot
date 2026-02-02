// Web API endpoints for the frontend UI.
//
// Provides REST endpoints for:
// - File tree browsing
// - Message history
// - Activity state (needs, wants, tasks)
// - System status (heads, hands)

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::bus::{Message, NeedPriority, Origin, Scope, respond};
use crate::history::Store;
use crate::runtime::RuntimeBus;

#[derive(Clone)]
pub struct WebApiState {
    pub bus: RuntimeBus,
    pub store: Arc<Store>,
    pub workspace_root: Arc<RwLock<PathBuf>>,
}

impl WebApiState {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, workspace_root: PathBuf) -> Self {
        Self {
            bus,
            store,
            workspace_root: Arc::new(RwLock::new(workspace_root)),
        }
    }
}

// File tree types
#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileEntry>>,
}

#[derive(Debug, Deserialize)]
pub struct FilesQuery {
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub path: String,
}

// Message types for API
#[derive(Debug, Serialize)]
pub struct ApiMessage {
    pub id: String,
    pub op: String,
    pub origin: String,
    pub sender: String,
    pub scope: String,
    pub data: serde_json::Value,
    pub reply_to: Option<String>,
    pub timestamp: i64,
}

impl From<Message> for ApiMessage {
    fn from(msg: Message) -> Self {
        let timestamp = msg
            .timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        Self {
            id: msg.id.to_string(),
            op: format!("{:?}", msg.op),
            origin: msg.origin.as_str().to_string(),
            sender: msg.sender,
            scope: msg.scope.to_string(),
            data: serde_json::to_value(&msg.data).unwrap_or(serde_json::Value::Null),
            reply_to: msg.reply_to.map(|u| u.to_string()),
            timestamp,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MessagesQuery {
    pub scope: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub scope: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiMemory {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ApiConclaveDetail {
    pub id: String,
    pub status: String,
    pub transcript: String,
    pub decision: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct ConclaveQuery {
    pub id: String,
}

// Handlers

async fn get_files(
    State(state): State<WebApiState>,
    Query(query): Query<FilesQuery>,
) -> Result<Json<Vec<FileEntry>>, StatusCode> {
    let workspace_root = state.workspace_root.read().await.clone();
    let rel_path = query.path.unwrap_or_default();

    let target_path = if rel_path.is_empty() {
        workspace_root.clone()
    } else {
        workspace_root.join(&rel_path)
    };

    if !target_path.starts_with(&workspace_root) {
        return Err(StatusCode::FORBIDDEN);
    }

    if !target_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let entries = read_dir_entries(&workspace_root, &target_path, 2)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(entries))
}

fn read_dir_entries(
    workspace_root: &PathBuf,
    path: &PathBuf,
    depth: usize,
) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files
        if name.starts_with('.') {
            continue;
        }

        // Follow symlinks to determine if target is a directory
        let entry_path = entry.path();
        let is_dir = entry_path.is_dir();

        let rel_path = entry_path
            .strip_prefix(workspace_root)
            .unwrap_or(&entry_path)
            .to_string_lossy()
            .to_string();

        let children = if is_dir && depth > 0 {
            Some(read_dir_entries(workspace_root, &entry_path, depth - 1).unwrap_or_default())
        } else {
            None
        };

        entries.push(FileEntry {
            name,
            path: rel_path,
            is_dir,
            children,
        });
    }

    // Sort: directories first, then alphabetically
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(entries)
}

async fn get_file_content(
    State(state): State<WebApiState>,
    Query(query): Query<FileQuery>,
) -> Result<Json<String>, StatusCode> {
    let workspace_root = state.workspace_root.read().await.clone();
    let target_path = workspace_root.join(&query.path);

    if !target_path.starts_with(&workspace_root) {
        return Err(StatusCode::FORBIDDEN);
    }

    let content = tokio::fs::read_to_string(&target_path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    Ok(Json(content))
}

async fn get_messages(
    State(state): State<WebApiState>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<Vec<ApiMessage>>, StatusCode> {
    let scope = query.scope.unwrap_or_else(|| "main".to_string());
    let limit = query.limit.unwrap_or(100);

    let messages = state
        .store
        .recent(&scope, limit)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let api_messages: Vec<ApiMessage> = messages.into_iter().map(ApiMessage::from).collect();
    Ok(Json(api_messages))
}

async fn get_self_identity(
    State(state): State<WebApiState>,
) -> Result<Json<ApiMemory>, StatusCode> {
    let workspace_root = state.workspace_root.read().await.clone();
    let workspace_dir = workspace_root
        .parent()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let path = workspace_dir.join("mind").join("self.md");

    let content = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) => state
            .store
            .get_conclave_self()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    };

    Ok(Json(ApiMemory { content }))
}

async fn get_ltm(State(state): State<WebApiState>) -> Result<Json<ApiMemory>, StatusCode> {
    let workspace_root = state.workspace_root.read().await.clone();
    let workspace_dir = workspace_root
        .parent()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let path = workspace_dir.join("mind").join("memory.md");

    let content = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) => state
            .store
            .get_head_ltm("conclave")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    };

    Ok(Json(ApiMemory { content }))
}

async fn get_conclave(
    State(state): State<WebApiState>,
    Query(query): Query<ConclaveQuery>,
) -> Result<Json<ApiConclaveDetail>, StatusCode> {
    let conclave = state
        .store
        .get_conclave(&query.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(ApiConclaveDetail {
        id: conclave.id,
        status: conclave.status,
        transcript: conclave.transcript,
        decision: conclave.decision,
        created_at: conclave.created_at,
    }))
}

async fn send_message(
    State(state): State<WebApiState>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<()>, StatusCode> {
    let scope_str = req.scope.unwrap_or_else(|| "main".to_string());
    let scope = Scope::from(scope_str.as_str());

    // Publish user message to scope for history/display
    let user_msg = respond::chat("_user", scope.clone(), &req.content).with_origin(Origin::Human);
    let user_msg_id = user_msg.id;
    state.bus.publish(user_msg).await;

    // Create a need for NeedService to dispatch to a head.
    // Use the same scope so the head can respond in-thread.
    let need_id = uuid::Uuid::new_v4().to_string();
    let need_msg = respond::need_request(
        "_user",
        scope.clone(),
        &need_id,
        "user",
        NeedPriority::Normal,
        &req.content,
        "",
    )
    .with_origin(Origin::Human)
    .with_reply_to(user_msg_id); // Correlate responses to user message

    state.bus.publish(need_msg).await;

    Ok(Json(()))
}

pub fn router(state: WebApiState) -> Router {
    Router::new()
        .route("/api/files", get(get_files))
        .route("/api/file", get(get_file_content))
        .route("/api/messages", get(get_messages))
        .route("/api/self", get(get_self_identity))
        .route("/api/ltm", get(get_ltm))
        .route("/api/conclave", get(get_conclave))
        .route("/api/send", post(send_message))
        .with_state(state)
}
