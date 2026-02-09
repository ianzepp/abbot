//! Frame Select - Conversation history query builder and parser
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module builds SQL queries against the frame store (frames table)
//! and parses stored frames into conversation items for API consumption.
//!
//! The refactor establishes chat:* syscalls as the canonical chat operations,
//! so conversation extraction looks for:
//! - kind="chat:user" / kind="chat:head" frames (persisted by dispatcher FrameStore)
//! - need:enqueue, need:fulfill, task:enqueue, task:complete for context
//!
//! WHY this exists: Conversation history is critical for LLM context building
//! and API responses. The query builder provides flexible filtering by room,
//! time range, actors, and frame types.

use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::SqlitePool;

use crate::kernel::Frame;

// =============================================================================
// TYPES
// =============================================================================

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrameSelectArgs {
    pub room: Option<String>,
    #[serde(default)]
    pub since_seq: Option<u64>,
    #[serde(default)]
    pub until_seq: Option<u64>,
    #[serde(default)]
    pub since_ts_ms: Option<i64>,
    #[serde(default)]
    pub until_ts_ms: Option<i64>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub ops: Option<Vec<String>>,
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    #[serde(default)]
    pub actors: Option<Vec<String>>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub frame_id: Option<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub include_frame: Option<bool>,
    #[serde(default)]
    pub include_json: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTask {
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationNeed {
    pub need_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub need: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationItem {
    pub seq: u64,
    pub ts_ms: i64,
    pub role: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub content: String,
    pub frame_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<ConversationTask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub need: Option<ConversationNeed>,
}

/// Parameter value for dynamic SQL binding.
///
/// WHY: SQLx does not have a rusqlite-style `Value` enum for dynamic param lists.
/// We use our own enum to build params, then bind them positionally.
#[derive(Debug, Clone)]
pub enum SqlParam {
    Integer(i64),
    Text(String),
}

// =============================================================================
// SQL QUERY BUILDER
// =============================================================================

/// Build a SQL query for frame store selection.
///
/// WHY: Centralizes query construction logic with validation and parameterization
/// to prevent SQL injection. Filters are combined with AND logic.
pub fn build_frame_select_sql(
    args: &FrameSelectArgs,
    order: &str,
    limit: i64,
) -> (String, Vec<SqlParam>) {
    let mut sql = String::from(
        "SELECT seq, ts_ms, op, name, actor, frame_id, parent_id, room, kind, reply_to, frame_json \
         FROM frames WHERE 1=1",
    );
    let mut params: Vec<SqlParam> = Vec::new();

    if let Some(since_seq) = args.since_seq {
        sql.push_str(" AND seq > ?");
        params.push(SqlParam::Integer(since_seq as i64));
    }
    if let Some(until_seq) = args.until_seq {
        sql.push_str(" AND seq <= ?");
        params.push(SqlParam::Integer(until_seq as i64));
    }
    if let Some(since_ts_ms) = args.since_ts_ms {
        sql.push_str(" AND ts_ms > ?");
        params.push(SqlParam::Integer(since_ts_ms));
    }
    if let Some(until_ts_ms) = args.until_ts_ms {
        sql.push_str(" AND ts_ms <= ?");
        params.push(SqlParam::Integer(until_ts_ms));
    }

    if let Some(room) = args
        .room
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND room = ?");
        params.push(SqlParam::Text(room.to_string()));
    }

    if let Some(parent_id) = args
        .parent_id
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND parent_id = ?");
        params.push(SqlParam::Text(parent_id.to_string()));
    }

    if let Some(frame_id) = args
        .frame_id
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND frame_id = ?");
        params.push(SqlParam::Text(frame_id.to_string()));
    }

    if let Some(reply_to) = args
        .reply_to
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND reply_to = ?");
        params.push(SqlParam::Text(reply_to.to_string()));
    }

    if let Some(query) = args
        .query
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND frame_json LIKE ?");
        params.push(SqlParam::Text(format!("%{query}%")));
    }

    if let Some(ops) = args.ops.as_ref().and_then(|v| {
        let xs: Vec<String> = v
            .iter()
            .map(|s| normalize_op(s))
            .filter(|s| !s.is_empty())
            .collect();
        (!xs.is_empty()).then_some(xs)
    }) {
        sql.push_str(" AND op IN (");
        for (i, op) in ops.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
            params.push(SqlParam::Text(op.clone()));
        }
        sql.push(')');
    }

    if let Some(kinds) = args.kinds.as_ref().and_then(|v| {
        let xs: Vec<String> = v
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (!xs.is_empty()).then_some(xs)
    }) {
        sql.push_str(" AND kind IN (");
        for (i, k) in kinds.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
            params.push(SqlParam::Text(k.clone()));
        }
        sql.push(')');
    }

    if let Some(actors) = args.actors.as_ref().and_then(|v| {
        let xs: Vec<String> = v
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (!xs.is_empty()).then_some(xs)
    }) {
        sql.push_str(" AND actor IN (");
        for (i, a) in actors.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
            params.push(SqlParam::Text(a.clone()));
        }
        sql.push(')');
    }

    sql.push_str(" ORDER BY seq ");
    sql.push_str(order);
    sql.push_str(" LIMIT ?");
    params.push(SqlParam::Integer(limit));

    (sql, params)
}

/// Bind a `Vec<SqlParam>` to a sqlx query and execute it, returning all rows.
pub async fn execute_frame_select(
    pool: &SqlitePool,
    sql: &str,
    params: &[SqlParam],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String> {
    let mut q = sqlx::query(sql);
    for p in params {
        q = match p {
            SqlParam::Integer(i) => q.bind(*i),
            SqlParam::Text(s) => q.bind(s.as_str()),
        };
    }
    q.fetch_all(pool)
        .await
        .map_err(|e| format!("frames query failed: {e}"))
}

/// Execute a conversation history query and parse results.
///
/// WHY: Provides a high-level API for extracting conversation items from the
/// frame store. Returns items + max_seq for cursor-based pagination.
pub async fn select_conversation(
    pool: &SqlitePool,
    args: &FrameSelectArgs,
) -> Result<(Vec<ConversationItem>, u64), String> {
    let limit = args.limit.unwrap_or(200).clamp(1, 2000) as i64;
    let order = match args
        .order
        .as_deref()
        .unwrap_or("asc")
        .to_lowercase()
        .as_str()
    {
        "asc" => "ASC",
        "desc" => "DESC",
        other => {
            return Err(format!(
                "invalid order '{other}' (expected 'asc' or 'desc')"
            ));
        }
    };

    let (sql, params) = build_frame_select_sql(args, order, limit);
    let rows = execute_frame_select(pool, &sql, &params).await?;

    let mut items: Vec<ConversationItem> = Vec::new();
    let mut max_seq: u64 = 0;
    for row in &rows {
        let seq: i64 = row.get(0);
        let ts_ms: i64 = row.get(1);
        let op: String = row.get(2);
        let name: Option<String> = row.try_get(3).ok();
        let actor: Option<String> = row.try_get(4).ok();
        let frame_id: String = row.get(5);
        let parent_id: Option<String> = row.try_get(6).ok();
        let room: Option<String> = row.try_get(7).ok();
        let kind: Option<String> = row.try_get(8).ok();
        let reply_to: Option<String> = row.try_get(9).ok();
        let frame_json: String = row.try_get(10).unwrap_or_else(|_| "{}".to_string());

        let seq_u = seq.max(0) as u64;
        max_seq = max_seq.max(seq_u);

        if let Some(item) = conversation_item_from_frame(
            seq_u,
            ts_ms,
            &op,
            name.as_deref(),
            actor.as_deref(),
            room.as_deref(),
            kind.as_deref(),
            reply_to.as_deref(),
            &frame_id,
            parent_id.as_deref(),
            &frame_json,
        ) {
            items.push(item);
        }
    }

    Ok((items, max_seq))
}

/// Parse a stored frame into a conversation item.
///
/// WHY: The frame store persists raw frames; conversation items are a higher-level
/// abstraction for API responses (role, content, task/need context).
///
/// TRADE-OFF: Only chat:*, need:*, and task:* frames are surfaced as conversation
/// items. Internal syscalls (fs:*, db:*) are stored but not conversation-visible.
#[allow(clippy::too_many_arguments)]
fn conversation_item_from_frame(
    seq: u64,
    ts_ms: i64,
    op: &str,
    name: Option<&str>,
    actor: Option<&str>,
    room: Option<&str>,
    kind: Option<&str>,
    reply_to: Option<&str>,
    frame_id: &str,
    parent_id: Option<&str>,
    frame_json: &str,
) -> Option<ConversationItem> {
    let frame: Frame = serde_json::from_str(frame_json).ok()?;
    let data = frame.data.as_ref()?;

    let room_val: Option<String> = room.map(|s| s.to_string()).or_else(|| {
        data.get("room")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    let mut frame_kind = kind.filter(|k| !k.is_empty()).map(|k| k.to_string());
    if frame_kind.is_none() {
        frame_kind = data
            .get("kind")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
    }

    // Derive kind for chat:message frames that lack an explicit kind field.
    // The Door's emit_chat_message() dispatches Req frames without a kind,
    // so we infer it from the actor prefix to enable conversation recovery.
    if frame_kind.is_none()
        && let Some(n) = name
        && n == "chat:message"
    {
        frame_kind = Some(match actor {
            Some(a) if a.starts_with("head/") || a.starts_with("room/") => "chat:head".to_string(),
            Some(a) if a == "user" || a.starts_with("human/") => "chat:user".to_string(),
            _ => format!("chat:{}", actor.unwrap_or("unknown")),
        });
    }

    // WHY role mapping: LLM APIs expect role (user/assistant/system), not kind
    let role = match frame_kind.as_deref() {
        Some("chat:head") => "assistant".to_string(),
        Some("chat:room") => "assistant".to_string(),
        Some("chat:user") => "user".to_string(),
        _ => role_from_actor(actor).to_string(),
    };

    if let Some(k) = frame_kind.as_deref() {
        if k == "chat:reset" {
            if !op.eq_ignore_ascii_case("req") {
                return None;
            }
            return Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "reset".to_string(),
                room: room_val.clone(),
                sender: actor.map(|s| s.to_string()),
                reply_to: reply_to.map(|s| s.to_string()),
                content: String::new(),
                frame_id: frame_id.to_string(),
                parent_id: parent_id.map(|s| s.to_string()),
                task: None,
                need: None,
            });
        }

        if k.starts_with("chat:") {
            if !op.eq_ignore_ascii_case("req") {
                return None;
            }
            let content = data
                .get("data")
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str())
                .or_else(|| data.get("content").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            if content.is_empty() {
                return None;
            }
            let sender = data
                .get("data")
                .and_then(|v| v.get("sender"))
                .and_then(|v| v.as_str())
                .or_else(|| data.get("sender").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
                .or_else(|| actor.map(|s| s.to_string()));
            return Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "chat".to_string(),
                room: room_val.clone(),
                sender,
                reply_to: reply_to.map(|s| s.to_string()),
                content,
                frame_id: frame_id.to_string(),
                parent_id: parent_id.map(|s| s.to_string()),
                task: None,
                need: None,
            });
        }
    }

    let name = name.or(frame.name.as_deref()).unwrap_or("");
    match name {
        "need:enqueue" => {
            let need_id = data.get("need_id").and_then(|v| v.as_str()).unwrap_or("");
            let need = data.get("need").and_then(|v| v.as_str()).unwrap_or("");
            let context = data.get("context").and_then(|v| v.as_str()).unwrap_or("");
            let priority = data.get("priority").and_then(|v| v.as_str()).unwrap_or("");
            let source = data.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let _room = room.or_else(|| data.get("room").and_then(|v| v.as_str()));
            let reply_to = reply_to.or_else(|| data.get("reply_to").and_then(|v| v.as_str()));
            if need_id.is_empty() && need.is_empty() {
                return None;
            }
            Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "need".to_string(),
                room: room_val.clone(),
                sender: actor.map(|s| s.to_string()),
                reply_to: reply_to.map(|s| s.to_string()),
                content: format!("need requested: {}", need),
                frame_id: frame_id.to_string(),
                parent_id: parent_id.map(|s| s.to_string()),
                task: None,
                need: Some(ConversationNeed {
                    need_id: need_id.to_string(),
                    need: Some(need.to_string()),
                    context: Some(context.to_string()),
                    priority: Some(priority.to_string()),
                    source: Some(source.to_string()),
                    status: "requested".to_string(),
                }),
            })
        }
        "need:fulfill" => {
            let need_id = data.get("need_id").and_then(|v| v.as_str()).unwrap_or("");
            if need_id.is_empty() {
                return None;
            }
            Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "need".to_string(),
                room: room_val.clone(),
                sender: actor.map(|s| s.to_string()),
                reply_to: reply_to.map(|s| s.to_string()),
                content: format!("need fulfilled: {}", need_id),
                frame_id: frame_id.to_string(),
                parent_id: parent_id.map(|s| s.to_string()),
                task: None,
                need: Some(ConversationNeed {
                    need_id: need_id.to_string(),
                    need: None,
                    context: None,
                    priority: None,
                    source: None,
                    status: "fulfilled".to_string(),
                }),
            })
        }
        "task:enqueue" => {
            let task_id = data.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
            let head_id = data.get("head_id").and_then(|v| v.as_str()).unwrap_or("");
            let prompt = data.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            let input = data.get("input").and_then(|v| v.as_str()).unwrap_or("");
            let _room = room.or_else(|| data.get("room").and_then(|v| v.as_str()));
            let reply_to = reply_to.or_else(|| data.get("reply_to").and_then(|v| v.as_str()));
            if task_id.is_empty() && prompt.is_empty() {
                return None;
            }
            Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "task".to_string(),
                room: room_val.clone(),
                sender: actor.map(|s| s.to_string()),
                reply_to: reply_to.map(|s| s.to_string()),
                content: format!("task {} requested: {}", task_id, prompt),
                frame_id: frame_id.to_string(),
                parent_id: parent_id.map(|s| s.to_string()),
                task: Some(ConversationTask {
                    task_id: task_id.to_string(),
                    head_id: Some(head_id.to_string()),
                    prompt: Some(prompt.to_string()),
                    input: Some(input.to_string()),
                    ok: None,
                    summary: None,
                    status: "requested".to_string(),
                }),
                need: None,
            })
        }
        "task:complete" => {
            let task_id = data.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
            let ok = data.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            let summary = data.get("summary").and_then(|v| v.as_str()).unwrap_or("");
            if task_id.is_empty() {
                return None;
            }

            let _room = room.or_else(|| data.get("room").and_then(|v| v.as_str()));
            let reply_to = reply_to.or_else(|| data.get("reply_to").and_then(|v| v.as_str()));

            let status = if ok { "completed" } else { "failed" }.to_string();
            Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "task".to_string(),
                room: room_val.clone(),
                sender: actor.map(|s| s.to_string()),
                reply_to: reply_to.map(|s| s.to_string()),
                content: format!("task {} {}: {}", task_id, status, summary),
                frame_id: frame_id.to_string(),
                parent_id: parent_id.map(|s| s.to_string()),
                task: Some(ConversationTask {
                    task_id: task_id.to_string(),
                    head_id: None,
                    prompt: None,
                    input: None,
                    ok: Some(ok),
                    summary: Some(summary.to_string()),
                    status,
                }),
                need: None,
            })
        }
        _ => None,
    }
}

/// Normalize frame op string to canonical case.
pub fn normalize_op(s: &str) -> String {
    let v = s.trim();
    if v.is_empty() {
        return String::new();
    }

    match v.to_lowercase().as_str() {
        "req" => "Req".to_string(),
        "cancel" => "Cancel".to_string(),
        "ok" => "Ok".to_string(),
        "error" => "Error".to_string(),
        "done" => "Done".to_string(),
        "item" => "Item".to_string(),
        "bytes" => "Bytes".to_string(),
        "event" => "Event".to_string(),
        "progress" => "Progress".to_string(),
        _ => v.to_string(),
    }
}

/// Map actor prefix to LLM role.
fn role_from_actor(actor: Option<&str>) -> &'static str {
    let Some(actor) = actor else {
        return "user";
    };
    if actor.starts_with("head/") {
        "assistant"
    } else if actor.starts_with("system/") {
        "system"
    } else {
        "user"
    }
}
