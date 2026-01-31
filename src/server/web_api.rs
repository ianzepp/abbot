// Web API endpoints for the frontend UI.
//
// Provides REST endpoints for:
// - File tree browsing
// - Message history
// - Activity state (needs, wants, goals)
// - System status (heads, hands)

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
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
    pub sandbox_root: Arc<RwLock<PathBuf>>,
}

impl WebApiState {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, sandbox_root: PathBuf) -> Self {
        Self {
            bus,
            store,
            sandbox_root: Arc::new(RwLock::new(sandbox_root)),
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

// Activity types
#[derive(Debug, Serialize)]
pub struct ApiNeed {
    pub id: String,
    pub source: String,
    pub priority: String,
    pub need: String,
    pub context: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct ApiWant {
    pub id: String,
    pub want: String,
    pub context: String,
    pub priority: String,
    pub source: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct ApiGoal {
    pub id: String,
    pub head_id: String,
    pub goal: String,
    pub notify_scope: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiHeadInfo {
    pub head_id: String,
    pub state: ApiHeadState,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state")]
pub enum ApiHeadState {
    #[serde(rename = "available")]
    Available,
    #[serde(rename = "processing")]
    Processing { need_id: String },
}

#[derive(Debug, Serialize)]
pub struct ApiHandInfo {
    pub hand_id: String,
    pub state: ApiHandState,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state")]
pub enum ApiHandState {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "running")]
    Running {
        task_id: String,
        goal_id: String,
        head_id: String,
    },
}

#[derive(Debug, Serialize)]
pub struct ApiStatus {
    pub heads: Vec<ApiHeadInfo>,
    pub hands: Vec<ApiHandInfo>,
    pub need_queue_depth: usize,
    pub goal_queue_depth: usize,
    pub wants_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct WantsQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub scope: Option<String>,
}

// Handlers

async fn get_files(
    State(state): State<WebApiState>,
    Query(query): Query<FilesQuery>,
) -> Result<Json<Vec<FileEntry>>, StatusCode> {
    let sandbox_root = state.sandbox_root.read().await.clone();
    let rel_path = query.path.unwrap_or_default();
    
    let target_path = if rel_path.is_empty() {
        sandbox_root.clone()
    } else {
        sandbox_root.join(&rel_path)
    };

    if !target_path.starts_with(&sandbox_root) {
        return Err(StatusCode::FORBIDDEN);
    }

    if !target_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let entries = read_dir_entries(&sandbox_root, &target_path, 2)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(entries))
}

fn read_dir_entries(
    sandbox_root: &PathBuf,
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
            .strip_prefix(sandbox_root)
            .unwrap_or(&entry_path)
            .to_string_lossy()
            .to_string();

        let children = if is_dir && depth > 0 {
            Some(read_dir_entries(sandbox_root, &entry_path, depth - 1).unwrap_or_default())
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
    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    Ok(entries)
}

async fn get_file_content(
    State(state): State<WebApiState>,
    Query(query): Query<FileQuery>,
) -> Result<Json<String>, StatusCode> {
    let sandbox_root = state.sandbox_root.read().await.clone();
    let target_path = sandbox_root.join(&query.path);

    if !target_path.starts_with(&sandbox_root) {
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

async fn get_needs(
    State(_state): State<WebApiState>,
) -> Result<Json<Vec<ApiNeed>>, StatusCode> {
    // For now, return empty. In a full implementation, we'd query NeedService.
    Ok(Json(vec![]))
}

async fn get_wants(
    State(state): State<WebApiState>,
    Query(query): Query<WantsQuery>,
) -> Result<Json<Vec<ApiWant>>, StatusCode> {
    let limit = query.limit.unwrap_or(20);
    
    let wants = state
        .store
        .list_wants(limit)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let api_wants: Vec<ApiWant> = wants
        .into_iter()
        .map(|w| ApiWant {
            id: w.id,
            want: w.want,
            context: w.context,
            priority: w.priority,
            source: w.source,
            created_at: w.created_at,
        })
        .collect();

    Ok(Json(api_wants))
}

async fn get_goals(
    State(_state): State<WebApiState>,
) -> Result<Json<Vec<ApiGoal>>, StatusCode> {
    // For now, return empty. In a full implementation, we'd query GoalService.
    Ok(Json(vec![]))
}

async fn get_status(
    State(_state): State<WebApiState>,
) -> Result<Json<ApiStatus>, StatusCode> {
    // Return placeholder status. In full implementation, query services.
    Ok(Json(ApiStatus {
        heads: vec![
            ApiHeadInfo {
                head_id: "head-0".to_string(),
                state: ApiHeadState::Available,
            },
            ApiHeadInfo {
                head_id: "head-1".to_string(),
                state: ApiHeadState::Available,
            },
            ApiHeadInfo {
                head_id: "head-2".to_string(),
                state: ApiHeadState::Available,
            },
        ],
        hands: vec![
            ApiHandInfo {
                hand_id: "hand-0".to_string(),
                state: ApiHandState::Idle,
            },
            ApiHandInfo {
                hand_id: "hand-1".to_string(),
                state: ApiHandState::Idle,
            },
            ApiHandInfo {
                hand_id: "hand-2".to_string(),
                state: ApiHandState::Idle,
            },
            ApiHandInfo {
                hand_id: "hand-3".to_string(),
                state: ApiHandState::Idle,
            },
        ],
        need_queue_depth: 0,
        goal_queue_depth: 0,
        wants_count: 0,
    }))
}

async fn send_message(
    State(state): State<WebApiState>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<()>, StatusCode> {
    let scope_str = req.scope.unwrap_or_else(|| "main".to_string());
    let scope = Scope::from(scope_str.as_str());

    // Publish user message to scope for history/display
    let user_msg = respond::chat("_user", scope.clone(), &req.content)
        .with_origin(Origin::Human);
    let user_msg_id = user_msg.id;
    state.bus.publish(user_msg).await;

    // Create a need for NeedService to dispatch to a head
    let need_id = uuid::Uuid::new_v4().to_string();
    let need_msg = respond::need_request(
        "_user",
        Scope::from("@need_service"),
        &need_id,
        "user",
        NeedPriority::Normal,
        &req.content,
        "",
    )
    .with_origin(Origin::Human);

    // Use user_msg_id as reply_to so head responses correlate
    let need_msg = Message {
        id: user_msg_id,
        reply_to: Some(user_msg_id),
        ..need_msg
    };

    state.bus.publish(need_msg).await;

    Ok(Json(()))
}

pub fn router(state: WebApiState) -> Router {
    Router::new()
        .route("/api/files", get(get_files))
        .route("/api/file", get(get_file_content))
        .route("/api/messages", get(get_messages))
        .route("/api/needs", get(get_needs))
        .route("/api/wants", get(get_wants))
        .route("/api/goals", get(get_goals))
        .route("/api/status", get(get_status))
        .route("/api/send", post(send_message))
        .with_state(state)
}
