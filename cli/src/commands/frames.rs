//! Frames command - Query kernel frame logs from SQLite

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;
use sqlx::Row;
use sqlx::sqlite::SqlitePool;

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
    /// Output all frames as a greppable one-line-per-frame transcript
    Transcript {
        /// Filter by room name (exact match)
        #[arg(long)]
        room: Option<String>,
        /// Filter by actor (substring match)
        #[arg(long)]
        actor: Option<String>,
        /// Filter by syscall name (substring match)
        #[arg(long)]
        name: Option<String>,
        /// Only return frames with seq > N
        #[arg(long)]
        since: Option<i64>,
        /// Only return frames with seq <= N
        #[arg(long)]
        until: Option<i64>,
        /// Max frames to return (default: 500)
        #[arg(long, default_value = "500")]
        limit: usize,
    },
    /// Replay recent frames (excludes tick frames)
    Replay {
        /// Filter by event kind (e.g., chat:user, chat:assistant)
        kind: Option<String>,
        /// Filter by syscall name (substring match)
        #[arg(long)]
        name: Option<String>,
        /// Filter by frame op (exact match: req, ok, error, event, item)
        #[arg(long)]
        op: Option<String>,
        /// Only return frames with seq > N
        #[arg(long)]
        since_frame: Option<i64>,
        /// Number of frames to return
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

pub async fn run(
    cli_config: Option<PathBuf>,
    action: FramesAction,
    format: OutputFormat,
) -> Result<(), CliError> {
    config::init_app_config(cli_config.as_deref());

    let frames_db_path = abbot::runtime::app_config::default_frames_db_path()
        .ok_or_else(|| CliError::General("could not determine frames.db path".into()))?;

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

        FramesAction::Transcript {
            room,
            actor,
            name,
            since,
            until,
            limit,
        } => {
            let limit_clamped = limit.min(5000) as i64;

            let mut conditions: Vec<String> = Vec::new();
            let mut bind_values: Vec<String> = Vec::new();

            if let Some(ref r) = room {
                let idx = bind_values.len() + 1;
                conditions.push(format!("room = ?{idx}"));
                bind_values.push(r.clone());
            }
            if let Some(ref a) = actor {
                let idx = bind_values.len() + 1;
                conditions.push(format!("actor LIKE ?{idx}"));
                bind_values.push(format!("%{a}%"));
            }
            if let Some(ref n) = name {
                let idx = bind_values.len() + 1;
                conditions.push(format!("name LIKE ?{idx}"));
                bind_values.push(format!("%{n}%"));
            }
            if let Some(s) = since {
                let idx = bind_values.len() + 1;
                conditions.push(format!("seq > ?{idx}"));
                bind_values.push(s.to_string());
            }
            if let Some(u) = until {
                let idx = bind_values.len() + 1;
                conditions.push(format!("seq <= ?{idx}"));
                bind_values.push(u.to_string());
            }

            let where_clause = if conditions.is_empty() {
                "1=1".to_string()
            } else {
                conditions.join(" AND ")
            };
            let limit_idx = bind_values.len() + 1;
            let sql = format!(
                "SELECT seq, ts_ms, op, name, actor, room, kind, frame_id, frame_json \
                 FROM frames WHERE {where_clause} ORDER BY seq ASC LIMIT ?{limit_idx}"
            );

            let mut query = sqlx::query(&sql);
            for val in &bind_values {
                query = query.bind(val);
            }
            query = query.bind(limit_clamped);

            let rows = query
                .fetch_all(&pool)
                .await
                .map_err(|e| CliError::General(e.to_string()))?;

            for row in &rows {
                let seq: i64 = row.get(0);
                let ts_ms: i64 = row.get(1);
                let op: String = row.get::<Option<String>, _>(2).unwrap_or_default();
                let db_name: String = row.get::<Option<String>, _>(3).unwrap_or_default();
                let db_actor: String = row.get::<Option<String>, _>(4).unwrap_or_default();
                let db_room: String = row.get::<Option<String>, _>(5).unwrap_or_default();
                let db_kind: String = row.get::<Option<String>, _>(6).unwrap_or_default();
                let frame_id: String = row.get::<Option<String>, _>(7).unwrap_or_default();
                let frame_json: String = row.get::<String, _>(8);
                let frame: serde_json::Value =
                    serde_json::from_str(&frame_json).unwrap_or_default();

                let line = format_transcript_line(
                    seq, ts_ms, &op, &db_name, &db_actor, &db_room, &db_kind, &frame_id, &frame,
                );
                println!("{line}");
            }
        }

        FramesAction::Replay {
            kind,
            name,
            op,
            since_frame,
            limit,
        } => {
            let limit_i64 = limit as i64;

            // Build dynamic WHERE clause
            let mut conditions: Vec<String> = Vec::new();
            let mut bind_values: Vec<String> = Vec::new();

            if let Some(ref k) = kind {
                let pattern = k.replace('*', "%");
                let is_pattern = pattern.contains('%');
                if is_pattern {
                    conditions.push(format!(
                        "(kind LIKE ?{n} OR name LIKE ?{n})",
                        n = bind_values.len() + 1
                    ));
                    bind_values.push(pattern);
                } else {
                    conditions.push(format!(
                        "(kind = ?{n} OR name = ?{n})",
                        n = bind_values.len() + 1
                    ));
                    bind_values.push(k.clone());
                }
            }

            if let Some(ref n) = name {
                let idx = bind_values.len() + 1;
                conditions.push(format!("name LIKE ?{idx}"));
                bind_values.push(format!("%{n}%"));
            }

            if let Some(ref o) = op {
                let idx = bind_values.len() + 1;
                conditions.push(format!("op = ?{idx}"));
                bind_values.push(o.clone());
            }

            if let Some(sf) = since_frame {
                let idx = bind_values.len() + 1;
                conditions.push(format!("seq > ?{idx}"));
                bind_values.push(sf.to_string());
            }

            // Default filter: exclude tick noise when no filters specified
            if conditions.is_empty() {
                conditions.push(
                    "(name IS NULL OR name != 'tick') AND (kind IS NULL OR kind != 'SIGTICK')"
                        .to_string(),
                );
            }

            let where_clause = conditions.join(" AND ");
            let limit_idx = bind_values.len() + 1;
            let order = if since_frame.is_some() { "ASC" } else { "DESC" };
            let sql = format!(
                "SELECT seq, ts_ms, frame_json FROM frames WHERE {where_clause} ORDER BY seq {order} LIMIT ?{limit_idx}"
            );

            let mut query = sqlx::query(&sql);
            for val in &bind_values {
                query = query.bind(val);
            }
            query = query.bind(limit_i64);

            let rows = query
                .fetch_all(&pool)
                .await
                .map_err(|e| CliError::General(e.to_string()))?;

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
                    if since_frame.is_none() {
                        frames.reverse();
                    }
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
        ("req", "need:lease")
        | ("req", "task:lease")
        | ("req", "tick:subscribe")
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

        ("event", "chat:llm", "llm:begin") => {
            let model = data["model"].as_str().unwrap_or("?");
            let provider = data["provider"].as_str().unwrap_or("?");
            let n_messages = data["messages"].as_u64().unwrap_or(0);
            let n_tools = data["tools"].as_u64().unwrap_or(0);
            println!(
                "LLM call: **{}** via {} | {} messages, {} tools",
                model, provider, n_messages, n_tools
            );
        }

        ("event", "chat:llm", "llm:result") => {
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

        ("item", "chat:llm", _) => {
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
                        if actor.is_empty() { "Assistant" } else { actor },
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

        ("req", "chat:llm", _) => {
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

// =============================================================================
// TRANSCRIPT FORMATTING
// =============================================================================

#[allow(clippy::too_many_arguments)]
fn format_transcript_line(
    seq: i64,
    ts_ms: i64,
    op: &str,
    name: &str,
    actor: &str,
    room: &str,
    kind: &str,
    frame_id: &str,
    frame: &serde_json::Value,
) -> String {
    use chrono::{Local, TimeZone};

    let ts = Local.timestamp_millis_opt(ts_ms).single();
    let time_str = match ts {
        Some(t) => t.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
        None => format!("{}ms", ts_ms),
    };

    let short_id = if frame_id.len() > 8 {
        &frame_id[..8]
    } else {
        frame_id
    };

    let tail = transcript_content(op, name, kind, frame);

    format!(
        "{time_str} seq={seq} id={short_id} op={op} name={name} actor={actor} room={room}{tail}"
    )
}

fn escape_content(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn transcript_content(op: &str, name: &str, kind: &str, frame: &serde_json::Value) -> String {
    let data = &frame["data"];

    match op {
        "req" => transcript_content_req(name, data),
        "ok" => transcript_content_ok(data),
        "done" => String::new(),
        "error" => {
            let code = data["code"].as_str().unwrap_or("UNKNOWN");
            let msg = data["message"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(msg, 120));
            format!(" code={code} content=\"{escaped}\"")
        }
        "item" => transcript_content_item(name, data),
        "event" => transcript_content_event(kind, data),
        "progress" => String::new(),
        _ => {
            let compact = serde_json::to_string(data).unwrap_or_default();
            if compact != "null" && compact != "{}" {
                let escaped = escape_content(&truncate_content(&compact, 120));
                format!(" content=\"{escaped}\"")
            } else {
                String::new()
            }
        }
    }
}

fn transcript_content_req(name: &str, data: &serde_json::Value) -> String {
    match name {
        "chat:message" => {
            let content = data["content"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(content, 120));
            format!(" content=\"{escaped}\"")
        }
        "chat:llm" => {
            let n = data["messages"].as_array().map_or(0, |a| a.len());
            format!(" content=\"messages={n}\"")
        }
        "chat:status" => {
            let status = data["status"].as_str().unwrap_or("?");
            format!(" content=\"status={status}\"")
        }
        "chat:done" => {
            let reason = data["reason"]
                .as_str()
                .or_else(|| data["stop_reason"].as_str())
                .unwrap_or("");
            if reason.is_empty() {
                String::new()
            } else {
                format!(" content=\"reason={reason}\"")
            }
        }
        "chat:tool" => {
            let tool = data["name"].as_str().unwrap_or("?");
            let args = if data["arguments"].is_string() {
                data["arguments"].as_str().unwrap_or("").to_string()
            } else if data["arguments"].is_object() {
                serde_json::to_string(&data["arguments"]).unwrap_or_default()
            } else {
                String::new()
            };
            let escaped = escape_content(&truncate_content(&args, 80));
            format!(" content=\"tool={tool} args={escaped}\"")
        }
        "chat:tool_result" => {
            let tool = data["name"].as_str().unwrap_or("?");
            let is_error = data["is_error"].as_bool().unwrap_or(false);
            let content = data["content"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(content, 80));
            format!(" content=\"tool={tool} is_error={is_error} {escaped}\"")
        }
        "chat:cancel" => {
            let reason = data["reason"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(reason, 120));
            format!(" content=\"reason={escaped}\"")
        }
        "need:enqueue" => {
            let priority = data["priority"].as_str().unwrap_or("?");
            let text = data["need"]
                .as_str()
                .or_else(|| data["text"].as_str())
                .unwrap_or("");
            let escaped = escape_content(&truncate_content(text, 100));
            format!(" content=\"priority={priority} {escaped}\"")
        }
        "need:fulfill" => {
            let id = data["need_id"]
                .as_str()
                .or_else(|| data["id"].as_str())
                .unwrap_or("?");
            let short = if id.len() > 8 { &id[..8] } else { id };
            let summary = data["summary"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(summary, 100));
            format!(" content=\"need_id={short} {escaped}\"")
        }
        "task:enqueue" => {
            let prompt = data["prompt"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(prompt, 120));
            format!(" content=\"{escaped}\"")
        }
        "task:complete" => {
            let ok = data["ok"].as_bool().unwrap_or(false);
            let summary = data["summary"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(summary, 100));
            format!(" content=\"ok={ok} {escaped}\"")
        }
        "tool:register" => {
            let n = data["tools"].as_array().map_or(0, |a| a.len());
            format!(" content=\"tools={n}\"")
        }
        _ if name.starts_with("ems:") => {
            let table = data["table"].as_str().unwrap_or("?");
            format!(" content=\"table={table}\"")
        }
        _ => {
            let compact = serde_json::to_string(data).unwrap_or_default();
            if compact != "null" && compact != "{}" {
                let escaped = escape_content(&truncate_content(&compact, 120));
                format!(" content=\"{escaped}\"")
            } else {
                String::new()
            }
        }
    }
}

fn transcript_content_ok(data: &serde_json::Value) -> String {
    // Extract true-valued boolean keys as detail
    if let Some(obj) = data.as_object() {
        let truthy: Vec<&str> = obj
            .iter()
            .filter_map(|(k, v)| {
                if v.as_bool() == Some(true) {
                    Some(k.as_str())
                } else {
                    None
                }
            })
            .collect();
        if !truthy.is_empty() {
            return format!(" detail={}", truthy.join(","));
        }
    }
    String::new()
}

fn transcript_content_item(name: &str, data: &serde_json::Value) -> String {
    let item_type = data["type"].as_str().unwrap_or("");
    match (name, item_type) {
        ("chat:llm", "text_delta") => {
            let content = data["content"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(content, 120));
            format!(" content=\"{escaped}\"")
        }
        ("chat:llm", "tool_call") => {
            let tool = data["name"].as_str().unwrap_or("?");
            let args = if data["arguments"].is_string() {
                data["arguments"].as_str().unwrap_or("").to_string()
            } else if data["arguments"].is_object() {
                serde_json::to_string(&data["arguments"]).unwrap_or_default()
            } else {
                String::new()
            };
            let escaped = escape_content(&truncate_content(&args, 80));
            format!(" content=\"tool={tool} {escaped}\"")
        }
        ("chat:llm", "tool_result") => {
            let tool = data["name"].as_str().unwrap_or("?");
            let content = data["content"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(content, 80));
            format!(" content=\"tool={tool} {escaped}\"")
        }
        ("chat:llm", "thinking") => {
            let content = data["content"].as_str().unwrap_or("");
            let escaped = escape_content(&truncate_content(content, 120));
            format!(" content=\"{escaped}\"")
        }
        _ => {
            let compact = serde_json::to_string(data).unwrap_or_default();
            if compact != "null" && compact != "{}" {
                let escaped = escape_content(&truncate_content(&compact, 120));
                format!(" content=\"{escaped}\"")
            } else {
                String::new()
            }
        }
    }
}

fn transcript_content_event(kind: &str, data: &serde_json::Value) -> String {
    match kind {
        "llm:begin" => {
            let model = data["model"].as_str().unwrap_or("?");
            let provider = data["provider"].as_str().unwrap_or("?");
            let msgs = data["messages"].as_u64().unwrap_or(0);
            let tools = data["tools"].as_u64().unwrap_or(0);
            format!(" content=\"model={model} provider={provider} msgs={msgs} tools={tools}\"")
        }
        "llm:result" => {
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
            format!(" content=\"completion={completion} prompt={prompt} total={total}\"")
        }
        "chat.ack" => {
            let thread_id = data["thread_id"].as_str().unwrap_or("?");
            format!(" content=\"thread_id={thread_id}\"")
        }
        "hand:start" => {
            let tool = data["tool"]
                .as_str()
                .or_else(|| data["name"].as_str())
                .unwrap_or("?");
            format!(" content=\"tool={tool}\"")
        }
        "hand:end" => String::new(),
        _ if !kind.is_empty() => {
            format!(" content=\"kind={kind}\"")
        }
        _ => String::new(),
    }
}
