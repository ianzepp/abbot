use std::path::Path;

use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, Connection};
use serde::{Deserialize, Serialize};

use crate::kernel::Frame;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LogSelectArgs {
    pub scope: Option<String>,
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
    pub goal: Option<String>,
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
    pub scope: Option<String>,
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

pub fn build_log_select_sql(
    args: &LogSelectArgs,
    order: &str,
    limit: i64,
) -> (String, Vec<SqlValue>) {
    let mut sql = String::from(
        "SELECT seq, ts_ms, op, name, actor, frame_id, parent_id, scope, kind, reply_to, frame_json \
         FROM kernel_frames WHERE 1=1",
    );
    let mut params: Vec<SqlValue> = Vec::new();

    if let Some(since_seq) = args.since_seq {
        sql.push_str(" AND seq > ?");
        params.push(SqlValue::Integer(since_seq as i64));
    }
    if let Some(until_seq) = args.until_seq {
        sql.push_str(" AND seq <= ?");
        params.push(SqlValue::Integer(until_seq as i64));
    }
    if let Some(since_ts_ms) = args.since_ts_ms {
        sql.push_str(" AND ts_ms > ?");
        params.push(SqlValue::Integer(since_ts_ms));
    }
    if let Some(until_ts_ms) = args.until_ts_ms {
        sql.push_str(" AND ts_ms <= ?");
        params.push(SqlValue::Integer(until_ts_ms));
    }

    if let Some(scope) = args
        .scope
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND scope = ?");
        params.push(SqlValue::Text(scope.to_string()));
    }

    if let Some(parent_id) = args
        .parent_id
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND parent_id = ?");
        params.push(SqlValue::Text(parent_id.to_string()));
    }

    if let Some(frame_id) = args
        .frame_id
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND frame_id = ?");
        params.push(SqlValue::Text(frame_id.to_string()));
    }

    if let Some(reply_to) = args
        .reply_to
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND reply_to = ?");
        params.push(SqlValue::Text(reply_to.to_string()));
    }

    if let Some(query) = args
        .query
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        sql.push_str(" AND frame_json LIKE ?");
        params.push(SqlValue::Text(format!("%{query}%")));
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
            params.push(SqlValue::Text(op.clone()));
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
            params.push(SqlValue::Text(k.clone()));
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
            params.push(SqlValue::Text(a.clone()));
        }
        sql.push(')');
    }

    sql.push_str(" ORDER BY seq ");
    sql.push_str(order);
    sql.push_str(" LIMIT ?");
    params.push(SqlValue::Integer(limit));

    (sql, params)
}

pub fn select_conversation(
    db_path: &Path,
    args: &LogSelectArgs,
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

    let (sql, params) = build_log_select_sql(args, order, limit);

    let conn = Connection::open(db_path).map_err(|e| format!("log db open failed: {e}"))?;
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("log query prepare failed: {e}"))?;
    let mut rows = stmt
        .query(params_from_iter(params))
        .map_err(|e| format!("log query failed: {e}"))?;

    let mut items: Vec<ConversationItem> = Vec::new();
    let mut max_seq: u64 = 0;
    while let Some(row) = rows.next().map_err(|e| format!("log read failed: {e}"))? {
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

        let seq_u = seq.max(0) as u64;
        max_seq = max_seq.max(seq_u);

        if let Some(item) = conversation_item_from_frame(
            seq_u,
            ts_ms,
            &op,
            name.as_deref(),
            actor.as_deref(),
            scope.as_deref(),
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

fn conversation_item_from_frame(
    seq: u64,
    ts_ms: i64,
    op: &str,
    name: Option<&str>,
    actor: Option<&str>,
    scope: Option<&str>,
    kind: Option<&str>,
    reply_to: Option<&str>,
    frame_id: &str,
    parent_id: Option<&str>,
    frame_json: &str,
) -> Option<ConversationItem> {
    let frame: Frame = serde_json::from_str(frame_json).ok()?;
    let data = frame.data.as_ref()?;

    let mut frame_kind = kind.map(|k| k.to_string());
    if frame_kind.is_none() {
        frame_kind = data
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    let role = match frame_kind.as_deref() {
        Some("chat:head") => "assistant".to_string(),
        Some("chat:user") => "user".to_string(),
        _ => role_from_actor(actor).to_string(),
    };

    if let Some(k) = frame_kind.as_deref() {
        if k.starts_with("chat:") {
            if !op.eq_ignore_ascii_case("req") {
                return None;
            }
            let content = data
                .get("data")
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if content.is_empty() {
                return None;
            }
            let sender = data
                .get("data")
                .and_then(|v| v.get("sender"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| actor.map(|s| s.to_string()));
            return Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "chat".to_string(),
                scope: scope.map(|s| s.to_string()),
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
            let scope = scope.or_else(|| data.get("scope").and_then(|v| v.as_str()));
            let reply_to = reply_to.or_else(|| data.get("reply_to").and_then(|v| v.as_str()));
            if need_id.is_empty() && need.is_empty() {
                return None;
            }
            Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "need".to_string(),
                scope: scope.map(|s| s.to_string()),
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
                scope: scope.map(|s| s.to_string()),
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
            let goal = data.get("goal").and_then(|v| v.as_str()).unwrap_or("");
            let input = data.get("input").and_then(|v| v.as_str()).unwrap_or("");
            let scope = scope.or_else(|| data.get("scope").and_then(|v| v.as_str()));
            let reply_to = reply_to.or_else(|| data.get("reply_to").and_then(|v| v.as_str()));
            if task_id.is_empty() && goal.is_empty() {
                return None;
            }
            Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "task".to_string(),
                scope: scope.map(|s| s.to_string()),
                sender: actor.map(|s| s.to_string()),
                reply_to: reply_to.map(|s| s.to_string()),
                content: format!("task {} requested: {}", task_id, goal),
                frame_id: frame_id.to_string(),
                parent_id: parent_id.map(|s| s.to_string()),
                task: Some(ConversationTask {
                    task_id: task_id.to_string(),
                    head_id: Some(head_id.to_string()),
                    goal: Some(goal.to_string()),
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

            // Fall back to payload-indexed fields if the audit index columns are empty.
            let scope = scope.or_else(|| data.get("scope").and_then(|v| v.as_str()));
            let reply_to = reply_to.or_else(|| data.get("reply_to").and_then(|v| v.as_str()));

            let status = if ok { "completed" } else { "failed" }.to_string();
            Some(ConversationItem {
                seq,
                ts_ms,
                role,
                kind: "task".to_string(),
                scope: scope.map(|s| s.to_string()),
                sender: actor.map(|s| s.to_string()),
                reply_to: reply_to.map(|s| s.to_string()),
                content: format!("task {} {}: {}", task_id, status, summary),
                frame_id: frame_id.to_string(),
                parent_id: parent_id.map(|s| s.to_string()),
                task: Some(ConversationTask {
                    task_id: task_id.to_string(),
                    head_id: None,
                    goal: None,
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
        "redirect" => "Redirect".to_string(),
        "item" => "Item".to_string(),
        "bytes" => "Bytes".to_string(),
        "event" => "Event".to_string(),
        "progress" => "Progress".to_string(),
        _ => v.to_string(),
    }
}

fn role_from_actor(actor: Option<&str>) -> &'static str {
    let Some(actor) = actor else {
        return "user";
    };
    if actor.starts_with("head/") {
        "assistant"
    } else if actor.starts_with("human/") {
        "user"
    } else if actor.starts_with("system/") {
        "system"
    } else if actor.starts_with("hand/") {
        "user"
    } else {
        "user"
    }
}
