//! Tail command - Stream live frames from the daemon via WebSocket

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use crate::config;
use crate::error::CliError;

const RETRY_INTERVAL: Duration = Duration::from_secs(3);

pub async fn run(
    cli_config: Option<PathBuf>,
    addr: Option<String>,
    filter: Option<String>,
    show_ticks: bool,
) -> Result<(), CliError> {
    use abbot::runtime::AppConfig;
    use futures_util::StreamExt;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    config::init_app_config(cli_config.as_deref());

    let bind_addr = addr
        .or_else(|| AppConfig::global().server.addr.clone())
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let ws_url = format!("ws://{}/ws", bind_addr);

    let filter_pattern = filter.as_ref().map(|f| f.replace('*', ""));
    let filter_is_prefix = filter.as_ref().map(|f| f.ends_with('*')).unwrap_or(false);

    let mut need_header = true;
    let mut attempts = 0u32;

    loop {
        attempts += 1;

        if attempts == 1 {
            eprint!("Waiting for {}...", ws_url);
        } else {
            eprint!("\rWaiting for {}... (attempt {})", ws_url, attempts);
        }
        let _ = std::io::stderr().flush();

        let ws_stream = match connect_async(&ws_url).await {
            Ok((stream, _)) => stream,
            Err(_) => {
                tokio::time::sleep(RETRY_INTERVAL).await;
                continue;
            }
        };

        attempts = 0;
        let (_, mut read) = ws_stream.split();

        eprintln!("\rConnected. Streaming frames (Ctrl+C to stop)        ");

        if need_header {
            println!(
                "{:8}  {:6}  {:20}  {:8}  {:16}  DATA",
                "TIME", "OP", "NAME", "ROOM", "ACTOR"
            );
            println!("{}", "-".repeat(100));
            need_header = false;
        }

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let ws_msg: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    if ws_msg.get("type").and_then(|t| t.as_str()) != Some("frame") {
                        continue;
                    }

                    let Some(frame) = ws_msg.get("data") else {
                        continue;
                    };

                    let op = frame.get("op").and_then(|v| v.as_str()).unwrap_or("-");
                    let name = frame.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                    let actor = frame.get("actor").and_then(|v| v.as_str()).unwrap_or("-");
                    let data = frame.get("data");

                    let kind = data
                        .and_then(|d| d.get("kind"))
                        .and_then(|k| k.as_str())
                        .unwrap_or("");
                    if !show_ticks && kind == "SIGTICK" {
                        continue;
                    }

                    if let Some(ref pattern) = filter_pattern {
                        let matches = if filter_is_prefix {
                            name.starts_with(pattern) || kind.starts_with(pattern)
                        } else {
                            name == filter.as_deref().unwrap_or("")
                                || kind == filter.as_deref().unwrap_or("")
                        };
                        if !matches {
                            continue;
                        }
                    }

                    let room = frame
                        .get("room")
                        .or_else(|| data.and_then(|d| d.get("room")))
                        .and_then(|r| r.as_str())
                        .map(|r| truncate_str(r, 8))
                        .unwrap_or_default();

                    let data_preview = data
                        .map(|d| {
                            let s = d.to_string();
                            truncate_str(&s, 60)
                        })
                        .unwrap_or_default();

                    let time = chrono::Local::now().format("%H:%M:%S").to_string();

                    println!(
                        "{:8}  {:6}  {:20}  {:8}  {:16}  {}",
                        time,
                        op,
                        truncate_str(name, 20),
                        room,
                        truncate_str(actor, 16),
                        data_preview
                    );
                }
                Ok(Message::Close(_)) => {
                    eprintln!("Connection closed, reconnecting...");
                    break;
                }
                Err(_) => {
                    eprintln!("Connection lost, reconnecting...");
                    break;
                }
                _ => {}
            }
        }

        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }

    // Truncate without panicking on UTF-8 boundaries.
    if s.len() <= max_len {
        return s.to_string();
    }

    if max_len <= 3 {
        return safe_prefix_utf8(s, max_len).to_string();
    }

    let head = safe_prefix_utf8(s, max_len - 3);
    format!("{head}...")
}

fn safe_prefix_utf8(s: &str, max_bytes: usize) -> &str {
    let mut end = std::cmp::min(max_bytes, s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
