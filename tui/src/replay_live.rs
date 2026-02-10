//! Live frame replay — fetches persisted frames from the daemon HTTP API
//! and replays them into the TUI event loop with original inter-frame timing.

use serde::Deserialize;
use tokio::sync::mpsc;

use crate::ws::WsEvent;

// -- Admin API response types ------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct LogsResponse {
    #[allow(dead_code)]
    pub count: u64,
    pub items: Vec<LogItem>,
}

#[derive(Deserialize)]
pub(crate) struct LogItem {
    pub seq: u64,
    pub ts_ms: i64,
    pub op: Option<String>,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub room: Option<String>,
    #[allow(dead_code)]
    pub actor: Option<String>,
    pub frame: Option<serde_json::Value>,
}

// -- Frame → WsEvent mapping -------------------------------------------------

pub(crate) fn map_frame(item: &LogItem) -> Option<WsEvent> {
    let op = item.op.as_deref().unwrap_or("");
    let name = item.name.as_deref().unwrap_or("");
    let kind = item.kind.as_deref().unwrap_or("");
    let room = item.room.clone().unwrap_or_else(|| "main".to_string());
    let data = item.frame.as_ref().and_then(|f| f.get("data")).cloned();

    // chat:user → ReplayUser
    if kind == "chat:user" {
        let content = extract_string(&data, "content")
            .or_else(|| {
                data.as_ref()
                    .and_then(|d| d.get("data"))
                    .and_then(|inner| inner.get("content"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        if content.is_empty() {
            return None;
        }
        return Some(WsEvent::ReplayUser { room, content });
    }

    // mind:thought → ChatMind
    if name == "mind:thought" || kind == "mind:thought" {
        let content = extract_string(&data, "content").unwrap_or_default();
        if content.is_empty() {
            return None;
        }
        return Some(WsEvent::ChatMind { room, content });
    }

    // hand:start / hand:end → ChatStatus (tool activity)
    if kind == "hand:start" || kind == "hand:end" {
        let actor = extract_string(&data, "actor");
        let summary = extract_string(&data, "summary")
            .or_else(|| extract_string(&data, "command"))
            .or_else(|| extract_string(&data, "tool"));
        return Some(WsEvent::ChatStatus {
            room,
            status: "tool".to_string(),
            actor,
            tool: None,
            summary,
        });
    }

    // Item frames
    if op == "Item" {
        let data_type = data
            .as_ref()
            .and_then(|d| d.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match data_type {
            "text_delta" => {
                let content = extract_string(&data, "content")
                    .or_else(|| extract_string(&data, "text"))
                    .unwrap_or_default();
                if content.is_empty() {
                    return None;
                }
                return Some(WsEvent::ChatDelta { room, content });
            }
            "tool_call" => {
                let tool_name = extract_string(&data, "name").unwrap_or_else(|| "?".to_string());
                return Some(WsEvent::ChatTool {
                    room,
                    name: tool_name,
                });
            }
            "status" => {
                let status = extract_string(&data, "status").unwrap_or_default();
                let actor = extract_string(&data, "actor");
                let tool = extract_string(&data, "tool");
                let summary = extract_string(&data, "summary");
                return Some(WsEvent::ChatStatus {
                    room,
                    status,
                    actor,
                    tool,
                    summary,
                });
            }
            "done" => {
                return Some(WsEvent::ChatDone { room });
            }
            _ => {}
        }
    }

    // Done op with name=chat:message → ChatDone
    if op == "Done" && name == "chat:message" {
        return Some(WsEvent::ChatDone { room });
    }

    // Error op → ChatError
    if op == "Error" {
        let message = extract_string(&data, "message")
            .or_else(|| extract_string(&data, "error"))
            .unwrap_or_else(|| "Unknown error".to_string());
        return Some(WsEvent::ChatError { room, message });
    }

    None
}

fn extract_string(data: &Option<serde_json::Value>, key: &str) -> Option<String> {
    data.as_ref()
        .and_then(|d| d.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// -- HTTP fetch + paginated replay -------------------------------------------

async fn fetch_frames(
    addr: &str,
    since_seq: u64,
) -> Result<Vec<LogItem>, Box<dyn std::error::Error>> {
    let mut all_items: Vec<LogItem> = Vec::new();
    let mut cursor = since_seq;
    let page_limit = 2000u64;

    loop {
        let base = format!("http://{}/admin/logs", addr);
        let mut url = reqwest::Url::parse(&base)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("since_seq", &cursor.to_string());
            pairs.append_pair("order", "asc");
            pairs.append_pair("limit", &page_limit.to_string());
        }

        let resp = reqwest::get(url).await?.json::<LogsResponse>().await?;
        let count = resp.items.len() as u64;
        if resp.items.is_empty() {
            break;
        }

        let last_seq = resp.items.last().map(|i| i.seq).unwrap_or(cursor);
        all_items.extend(resp.items);

        // If we got fewer than the page limit, we've fetched everything.
        if count < page_limit {
            break;
        }
        cursor = last_seq;
    }

    Ok(all_items)
}

/// Fetch frames from `since_seq` onward and replay them into `event_tx`
/// with original inter-frame timing (capped at 2 seconds per gap).
pub async fn run_replay(addr: String, since_seq: u64, event_tx: mpsc::Sender<WsEvent>) {
    // Signal connected so the TUI renders normally.
    let _ = event_tx.send(WsEvent::Connected).await;

    let items = match fetch_frames(&addr, since_seq).await {
        Ok(items) => items,
        Err(_) => return,
    };

    let mut prev_ts: Option<i64> = None;

    for item in &items {
        // Compute inter-frame delay.
        if let Some(prev) = prev_ts {
            let delta_ms = (item.ts_ms - prev).max(0) as u64;
            let capped = delta_ms.min(2000);
            if capped > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(capped)).await;
            }
        }
        prev_ts = Some(item.ts_ms);

        if let Some(event) = map_frame(item)
            && event_tx.send(event).await.is_err()
        {
            return;
        }
    }

    // Replay complete — task returns, TUI stays open.
}
