//! Reconnect replay — fetches missed frames from daemon HTTP API and
//! converts them to WsEvents using the shared frame mapping.

use crate::replay_live::{LogsResponse, map_frame};
use crate::ws::WsEvent;

/// Fetch recent history for `room` since `since_ts` and return mapped events
/// plus the max timestamp seen (for watermark tracking).
///
/// On any error the function silently returns empty (replay is best-effort).
pub async fn fetch_history(addr: &str, room: &str, since_ts: i64) -> (Vec<WsEvent>, i64) {
    fetch_inner(addr, room, since_ts).await.unwrap_or_default()
}

async fn fetch_inner(
    addr: &str,
    room: &str,
    since_ts: i64,
) -> Result<(Vec<WsEvent>, i64), Box<dyn std::error::Error>> {
    let base = format!("http://{}/admin/logs", addr);
    let mut url = reqwest::Url::parse(&base)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("room", room);
        pairs.append_pair("order", "asc");
        pairs.append_pair("limit", "500");
        if since_ts > 0 {
            pairs.append_pair("since_ts_ms", &since_ts.to_string());
        }
    }

    let resp = reqwest::get(url).await?.json::<LogsResponse>().await?;

    let mut events: Vec<WsEvent> = Vec::new();
    let mut max_ts = since_ts;

    for item in &resp.items {
        if item.ts_ms > max_ts {
            max_ts = item.ts_ms;
        }
        if let Some(event) = map_frame(item) {
            events.push(event);
        }
    }

    Ok((events, max_ts))
}
