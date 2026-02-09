//! Reconnect replay — fetches missed history from daemon HTTP API.

use std::collections::BTreeMap;

use serde::Deserialize;

/// A single replayed message to insert into a room.
pub struct ReplayEntry {
    #[allow(dead_code)]
    pub ts_ms: i64,
    pub kind: ReplayKind,
    pub content: String,
}

/// Whether the replayed message was from the user or the assistant.
pub enum ReplayKind {
    User,
    Assistant,
}

// -- HTTP response types (match daemon GET /admin/logs) -----------------------

#[derive(Deserialize)]
struct LogsResponse {
    #[allow(dead_code)]
    count: u64,
    items: Vec<LogItem>,
}

#[derive(Deserialize)]
struct LogItem {
    seq: i64,
    ts_ms: i64,
    kind: Option<String>,
    reply_to: Option<String>,
    frame: Option<FrameJson>,
}

#[derive(Deserialize)]
struct FrameJson {
    data: Option<serde_json::Value>,
}

// -- Public API ---------------------------------------------------------------

/// Fetch recent history for `scope` from the daemon's admin endpoint.
///
/// Returns entries sorted chronologically. On any error the function
/// silently returns an empty vec (replay is best-effort).
pub async fn fetch_history(addr: &str, scope: &str, since_ts: i64) -> Vec<ReplayEntry> {
    let entries: Vec<ReplayEntry> = fetch_inner(addr, scope, since_ts).await.unwrap_or_default();
    entries
}

async fn fetch_inner(
    addr: &str,
    scope: &str,
    since_ts: i64,
) -> Result<Vec<ReplayEntry>, Box<dyn std::error::Error>> {
    let mut url = format!(
        "http://{}/admin/logs?scope={}&order=asc&limit=200",
        addr, scope
    );
    if since_ts > 0 {
        url.push_str(&format!("&since_ts_ms={}", since_ts));
    }

    let resp = reqwest::get(&url).await?.json::<LogsResponse>().await?;

    let mut entries: Vec<ReplayEntry> = Vec::new();

    // Group consecutive text_delta items by reply_to into single assistant messages.
    // BTreeMap<reply_to, (earliest_ts, accumulated_text, earliest_seq)>
    let mut assistant_buf: BTreeMap<String, (i64, String, i64)> = BTreeMap::new();

    for item in &resp.items {
        let kind_str = item.kind.as_deref().unwrap_or("");

        if kind_str == "chat:user" {
            // Flush any pending assistant buffer first (maintains ordering).
            flush_assistant_buf(&mut assistant_buf, &mut entries);

            let content = extract_user_content(item);
            if !content.is_empty() {
                entries.push(ReplayEntry {
                    ts_ms: item.ts_ms,
                    kind: ReplayKind::User,
                    content,
                });
            }
        } else if is_text_delta(item) {
            let delta = extract_delta_text(item);
            if !delta.is_empty() {
                let reply_key = item
                    .reply_to
                    .clone()
                    .unwrap_or_else(|| format!("_seq_{}", item.seq));
                let entry = assistant_buf
                    .entry(reply_key)
                    .or_insert_with(|| (item.ts_ms, String::new(), item.seq));
                entry.1.push_str(&delta);
            }
        }
        // Ignore all other frame kinds (tool calls, events, etc.)
    }

    // Flush remaining assistant text.
    flush_assistant_buf(&mut assistant_buf, &mut entries);

    // Already ordered by seq (ascending) from the API.
    Ok(entries)
}

fn flush_assistant_buf(
    buf: &mut BTreeMap<String, (i64, String, i64)>,
    entries: &mut Vec<ReplayEntry>,
) {
    // Sort by earliest seq so ordering is preserved.
    let mut pending: Vec<(i64, i64, String)> = std::mem::take(buf)
        .into_values()
        .map(|(ts, text, seq)| (seq, ts, text))
        .collect();
    pending.sort_by_key(|(seq, _, _)| *seq);
    for (_, ts, text) in pending {
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            entries.push(ReplayEntry {
                ts_ms: ts,
                kind: ReplayKind::Assistant,
                content: trimmed,
            });
        }
    }
}

/// Check if a log item represents a text_delta (assistant streaming chunk).
fn is_text_delta(item: &LogItem) -> bool {
    if let Some(frame) = &item.frame
        && let Some(data) = &frame.data
        && data.get("type").and_then(|v| v.as_str()) == Some("text_delta")
    {
        return true;
    }
    false
}

/// Extract the text content from a text_delta frame.
fn extract_delta_text(item: &LogItem) -> String {
    if let Some(frame) = &item.frame
        && let Some(data) = &frame.data
        && let Some(text) = data.get("text").and_then(|v| v.as_str())
    {
        return text.to_string();
    }
    String::new()
}

/// Extract user message content from a chat:user frame.
fn extract_user_content(item: &LogItem) -> String {
    if let Some(frame) = &item.frame
        && let Some(data) = &frame.data
    {
        // Try data.content first (direct string)
        if let Some(s) = data.get("content").and_then(|v| v.as_str()) {
            return s.to_string();
        }
        // Try data.data.content (nested)
        if let Some(inner) = data.get("data")
            && let Some(s) = inner.get("content").and_then(|v| v.as_str())
        {
            return s.to_string();
        }
    }
    String::new()
}
