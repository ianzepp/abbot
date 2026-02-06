//! Frames command - Query kernel frame logs from SQLite

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::config;
use crate::error::CliError;
use crate::output::{OutputFormat, print_value};

#[derive(Debug, Subcommand, Clone)]
pub enum FramesAction {
    /// Get a frame by its UUID
    Get {
        /// Frame UUID
        id: String,
    },
    /// Replay recent frames (excludes tick frames)
    Replay {
        /// Filter by event kind (e.g., chat:user, chat:assistant)
        kind: Option<String>,
        /// Number of frames to return
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

pub async fn run(cli_config: Option<PathBuf>, action: FramesAction, format: OutputFormat) -> Result<(), CliError> {
    use abbot::runtime::AppConfig;
    use abbot::runtime::app_config::WorkspacePaths;

    config::init_app_config(cli_config.as_deref());

    let workspace = AppConfig::global()
        .workspace_path()
        .map_err(|e| CliError::General(format!("workspace configuration error: {}", e)))?;
    let paths = WorkspacePaths::new(workspace);
    let frames_db_path = paths.frames_db;

    if !frames_db_path.exists() {
        eprintln!("Frames database not found: {}", frames_db_path.display());
        std::process::exit(1);
    }

    let pool = SqlitePool::connect(&format!("sqlite:{}?mode=ro", frames_db_path.display()))
        .await
        .map_err(|e| CliError::General(e.to_string()))?;

    match action {
        FramesAction::Get { id } => {
            let row = sqlx::query("SELECT frame_json FROM frames WHERE frame_id = ?1 LIMIT 1")
                .bind(&id)
                .fetch_optional(&pool)
                .await
                .map_err(|e| CliError::General(e.to_string()))?;

            match row {
                Some(row) => {
                    let frame_json: String = row.get::<String, _>(0);
                    let frame: serde_json::Value = serde_json::from_str(&frame_json)?;
                    print_value(&frame, format);
                }
                None => {
                    eprintln!("Frame not found: {}", id);
                    std::process::exit(1);
                }
            }
        }

        FramesAction::Replay {
            kind,
            limit,
        } => {
            let limit_i64 = limit as i64;

            let rows = if let Some(ref k) = kind {
                let pattern = k.replace('*', "%");
                let is_pattern = pattern.contains('%');
                if is_pattern {
                    sqlx::query(
                        "SELECT seq, ts_ms, frame_json FROM frames
                         WHERE kind LIKE ?1 OR name LIKE ?1
                         ORDER BY seq DESC
                         LIMIT ?2",
                    )
                    .bind(&pattern)
                    .bind(limit_i64)
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| CliError::General(e.to_string()))?
                } else {
                    sqlx::query(
                        "SELECT seq, ts_ms, frame_json FROM frames
                         WHERE kind = ?1 OR name = ?1
                         ORDER BY seq DESC
                         LIMIT ?2",
                    )
                    .bind(k)
                    .bind(limit_i64)
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| CliError::General(e.to_string()))?
                }
            } else {
                sqlx::query(
                    "SELECT seq, ts_ms, frame_json FROM frames
                     WHERE (name IS NULL OR name != 'tick')
                       AND (kind IS NULL OR kind != 'SIGTICK')
                     ORDER BY seq DESC
                     LIMIT ?1",
                )
                .bind(limit_i64)
                .fetch_all(&pool)
                .await
                .map_err(|e| CliError::General(e.to_string()))?
            };

            let mut frames: Vec<(i64, i64, serde_json::Value)> = Vec::new();
            for row in &rows {
                let seq: i64 = row.get::<i64, _>(0);
                let ts_ms: i64 = row.get::<i64, _>(1);
                let frame_json: String = row.get::<String, _>(2);
                let frame: serde_json::Value = serde_json::from_str(&frame_json)?;
                frames.push((seq, ts_ms, frame));
            }

            let resolved = format.resolve();
            match resolved {
                OutputFormat::Pretty => {
                    frames.reverse();
                    for (seq, ts_ms, frame) in &frames {
                        print_frame_markdown(*seq, *ts_ms, frame);
                    }
                }
                OutputFormat::Json => {
                    let json_frames: Vec<serde_json::Value> = frames
                        .into_iter()
                        .map(|(seq, ts_ms, frame)| {
                            json!({
                                "seq": seq,
                                "ts_ms": ts_ms,
                                "frame": frame,
                            })
                        })
                        .collect();
                    print_value(&json!(json_frames), format);
                }
                OutputFormat::Auto => unreachable!(),
            }
        }
    }

    Ok(())
}

fn truncate_content(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{}...", truncated)
}

fn print_frame_markdown(seq: i64, ts_ms: i64, frame: &serde_json::Value) {
    use chrono::{Local, TimeZone};

    let op = frame["op"].as_str().unwrap_or("");
    let name = frame["name"].as_str().unwrap_or("");
    let data = &frame["data"];

    let kind = frame["kind"]
        .as_str()
        .or_else(|| data["kind"].as_str())
        .unwrap_or("");

    let actor = frame["actor"]
        .as_str()
        .or_else(|| data["data"]["sender"].as_str())
        .unwrap_or("");

    // Skip noise
    match (op, name) {
        ("req", "need:lease") | ("req", "task:lease") | ("req", "tick:subscribe")
        | ("req", "tool:register") => return,
        ("ok", _) | ("done", _) => return,
        _ => {}
    }

    if op == "event"
        && matches!(
            kind,
            "chat:user" | "chat:head" | "chat:tool_result" | "thinking" | "chat:reset"
        )
    {
        return;
    }

    if op == "req"
        && matches!(
            name,
            "chat:message" | "chat:tool" | "chat:done" | "chat:tool_result"
        )
    {
        return;
    }

    if op == "item" && matches!(name, "chat:message" | "chat:tool") {
        return;
    }

    let ts = Local.timestamp_millis_opt(ts_ms).single();
    let time_str = match ts {
        Some(t) => t.format("%H:%M:%S%.3f").to_string(),
        None => format!("{}ms", ts_ms),
    };

    let context = if !actor.is_empty() {
        actor.to_string()
    } else if !kind.is_empty() {
        kind.to_string()
    } else {
        String::new()
    };
    if context.is_empty() {
        println!("\n### [{}] #{} {} {}", time_str, seq, op, name);
    } else {
        println!(
            "\n### [{}] #{} {} {} ({})",
            time_str, seq, op, name, context
        );
    }

    match (op, name, kind) {
        ("error", _, _) => {
            let code = data["code"].as_str().unwrap_or("UNKNOWN");
            let retryable = data["retryable"].as_bool().unwrap_or(false);
            let msg = data["message"].as_str().unwrap_or("");
            let retry_str = if retryable { "retryable" } else { "fatal" };
            println!(
                "**Error** `{}` ({}) {}",
                code,
                retry_str,
                truncate_content(msg, 200)
            );
        }

        ("event", "llm:chat", "llm:begin") => {
            let model = data["model"].as_str().unwrap_or("?");
            let provider = data["provider"].as_str().unwrap_or("?");
            let n_messages = data["messages"].as_u64().unwrap_or(0);
            let n_tools = data["tools"].as_u64().unwrap_or(0);
            println!(
                "LLM call: **{}** via {} | {} messages, {} tools",
                model, provider, n_messages, n_tools
            );
        }

        ("event", "llm:chat", "llm:result") => {
            let usage = &data["usage"];
            let completion = usage["completion_tokens"]
                .as_u64()
                .or_else(|| data["completion_tokens"].as_u64())
                .unwrap_or(0);
            let prompt = usage["prompt_tokens"]
                .as_u64()
                .or_else(|| data["prompt_tokens"].as_u64())
                .unwrap_or(0);
            let total = completion + prompt;
            println!(
                "LLM result: {} completion + {} prompt = **{} total tokens**",
                completion, prompt, total
            );
        }

        ("item", "llm:chat", _) => {
            let item_type = data["type"].as_str().unwrap_or("");
            match item_type {
                "thinking" => {
                    let content = data["content"].as_str().unwrap_or("");
                    println!("> *thinking:* {}", truncate_content(content, 600));
                }
                "text_delta" => {
                    let content = data["content"].as_str().unwrap_or("");
                    println!(
                        "**{}:** {}",
                        if actor.is_empty() {
                            "Assistant"
                        } else {
                            actor
                        },
                        truncate_content(content, 600)
                    );
                }
                "tool_call" => {
                    let tool_name = data["name"].as_str().unwrap_or("?");
                    let args = if data["arguments"].is_string() {
                        data["arguments"].as_str().unwrap_or("").to_string()
                    } else if data["arguments"].is_object() {
                        serde_json::to_string(&data["arguments"]).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    println!(
                        "**Tool call:** `{}` {}",
                        tool_name,
                        truncate_content(&args, 200)
                    );
                }
                "tool_result" => {
                    let tool_name = data["name"].as_str().unwrap_or("?");
                    let content = data["content"].as_str().unwrap_or("");
                    println!(
                        "**Tool result** (`{}`): {}",
                        tool_name,
                        truncate_content(content, 200)
                    );
                }
                _ => {
                    let compact = serde_json::to_string(data).unwrap_or_default();
                    println!("{}", truncate_content(&compact, 300));
                }
            }
        }

        ("event", "mind:conclave", "mind:round_start") => {
            let round = data["round"].as_u64().unwrap_or(0);
            let round_type = data["type"].as_str().unwrap_or("?");
            println!("**Conclave round {}** ({})", round, round_type);
        }

        ("event", "mind:autonomy", "mind:start") => {
            let wake = data["wake"]
                .as_str()
                .or_else(|| data["wake_reason"].as_str())
                .unwrap_or("?");
            println!("**Autonomy meeting started** | wake={}", wake);
        }

        ("item", "chat:done", _) => {
            let reason = data["reason"]
                .as_str()
                .or_else(|| data["stop_reason"].as_str())
                .unwrap_or("?");
            println!("**Turn complete** ({})", reason);
        }

        ("req", "need:enqueue", _) => {
            let priority = data["priority"].as_str().unwrap_or("?");
            let text = data["need"]
                .as_str()
                .or_else(|| data["text"].as_str())
                .unwrap_or("");
            println!(
                "**Need created** ({}): {}",
                priority,
                truncate_content(text, 200)
            );
        }

        ("req", "need:fulfill", _) => {
            let id = data["need_id"]
                .as_str()
                .or_else(|| data["id"].as_str())
                .unwrap_or("?");
            let short_id = if id.len() > 8 { &id[..8] } else { id };
            let summary = data["summary"].as_str().unwrap_or("");
            println!(
                "**Need fulfilled** `{}`: {}",
                short_id,
                truncate_content(summary, 200)
            );
        }

        ("req", "task:enqueue", _) => {
            let head_id = data["head_id"].as_str().unwrap_or("?");
            let short_head = if head_id.len() > 8 {
                &head_id[..8]
            } else {
                head_id
            };
            let prompt = data["prompt"].as_str().unwrap_or("");
            let input = data["input"].as_str().unwrap_or("");
            let combined = format!("{} {}", prompt, input);
            println!(
                "**Task dispatched** to {}: {}",
                short_head,
                truncate_content(&combined, 200)
            );
        }

        ("req", "task:complete", _) => {
            let ok = data["ok"].as_bool().unwrap_or(false);
            let summary = data["summary"].as_str().unwrap_or("");
            println!(
                "**Task complete** (ok: {}): {}",
                ok,
                truncate_content(summary, 200)
            );
        }

        ("req", "llm:chat", _) => {
            let n_messages = data["messages"].as_array().map_or(0, |a| a.len());
            println!("**LLM request**: {} messages", n_messages);
        }

        _ => {
            let compact = serde_json::to_string(data).unwrap_or_default();
            if compact != "null" && compact != "{}" {
                println!("{}", truncate_content(&compact, 300));
            }
        }
    }
}
