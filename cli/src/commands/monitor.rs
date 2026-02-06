//! Monitor command - Stream frames from the daemon via WebSocket

use std::path::PathBuf;

use crate::config;
use crate::error::CliError;

pub async fn run(
    cli_config: Option<PathBuf>,
    addr: Option<String>,
    filter: Option<String>,
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
    let filter_is_prefix = filter
        .as_ref()
        .map(|f| f.ends_with('*'))
        .unwrap_or(false);

    eprintln!("Connecting to {}...", ws_url);

    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .map_err(|e| CliError::General(e.to_string()))?;
    let (_, mut read) = ws_stream.split();

    eprintln!("Connected. Streaming frames (Ctrl+C to stop)\n");

    println!(
        "{:8}  {:6}  {:20}  {:6}  {:16}  {}",
        "TIME", "OP", "NAME", "SCOPE", "ACTOR", "DATA"
    );
    println!("{}", "-".repeat(100));

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
                if kind == "SIGTICK" {
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

                let scope = data
                    .and_then(|d| d.get("scope"))
                    .and_then(|s| s.as_str())
                    .map(|s| {
                        if let Some(hash) = s.strip_prefix("session/") {
                            format!("@{}", &hash[..4.min(hash.len())])
                        } else {
                            format!("#{}", s)
                        }
                    })
                    .unwrap_or_default();

                let data_preview = data
                    .map(|d| {
                        let s = d.to_string();
                        if s.len() > 60 {
                            format!("{}...", &s[..60])
                        } else {
                            s
                        }
                    })
                    .unwrap_or_default();

                let time = chrono::Local::now().format("%H:%M:%S").to_string();

                println!(
                    "{:8}  {:6}  {:20}  {:6}  {:16}  {}",
                    time,
                    op,
                    truncate_str(name, 20),
                    truncate_str(&scope, 6),
                    truncate_str(actor, 16),
                    data_preview
                );
            }
            Ok(Message::Close(_)) => {
                eprintln!("\nConnection closed");
                break;
            }
            Err(e) => {
                eprintln!("\nWebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 1])
    }
}
