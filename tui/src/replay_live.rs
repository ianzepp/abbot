//! Live frame replay — fetches persisted frames from the daemon HTTP API
//! and replays them into the TUI event loop with original inter-frame timing.

use serde::Deserialize;
use tokio::sync::mpsc;

use crate::ws::{self, WsEvent};

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
    #[allow(dead_code)]
    pub op: Option<String>,
    #[allow(dead_code)]
    pub name: Option<String>,
    #[allow(dead_code)]
    pub kind: Option<String>,
    #[allow(dead_code)]
    pub room: Option<String>,
    #[allow(dead_code)]
    pub actor: Option<String>,
    pub frame: Option<serde_json::Value>,
}

// -- Frame → WsEvent mapping -------------------------------------------------

pub(crate) fn map_frame(item: &LogItem) -> Option<WsEvent> {
    // Parse the raw JSON frame into our local Frame type and delegate to map_ws_frame.
    let frame_json = item.frame.as_ref()?;
    let mut frame: ws::Frame = serde_json::from_value(frame_json.clone()).ok()?;

    // Inject seq into trace so that replay consumers can track watermarks.
    // The admin API provides seq per LogItem but it's not in the frame itself.
    let trace = frame.trace.get_or_insert_with(|| serde_json::json!({}));
    if let Some(obj) = trace.as_object_mut() {
        obj.insert("seq".to_string(), serde_json::json!(item.seq));
    }

    let event = ws::map_ws_frame(&frame)?;

    // Patch seq into events that support it (ChatDelta, ReplayUser).
    Some(match event {
        WsEvent::ChatDelta {
            room,
            content,
            seq: _,
        } => WsEvent::ChatDelta {
            room,
            content,
            seq: Some(item.seq),
        },
        WsEvent::ReplayUser {
            room,
            content,
            seq: _,
        } => WsEvent::ReplayUser {
            room,
            content,
            seq: item.seq,
        },
        other => other,
    })
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
