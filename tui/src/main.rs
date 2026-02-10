//! abbot-tui — multi-room chat TUI for the Abbot daemon.
//!
//! Connects to the daemon via WebSocket for real-time chat with streaming
//! responses, tool call visibility, and multi-room chat tabs.

mod app;
mod markdown;
mod replay;
mod replay_live;
mod room;
mod theme;
mod tools;
mod ui;
mod ws;

use std::io;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use app::{App, AppView, ChatEntry, EntryKind, Mode};
use ws::{WsEvent, WsInMessage};

// =============================================================================
// CLI
// =============================================================================

#[derive(Parser)]
#[command(name = "abbot-tui")]
#[command(about = "Multi-room chat TUI for Abbot")]
struct Cli {
    /// Daemon server address (host:port)
    #[arg(long, default_value = "127.0.0.1:8080")]
    addr: String,

    /// Initial chat room
    #[arg(long, default_value = "main")]
    room: String,

    /// Replay frames from sequence N with original timing (no WebSocket)
    #[arg(long)]
    replay_live: Option<u64>,

    /// Enable developer-only views (Frames, EMS)
    #[arg(long)]
    developer: bool,
}

// =============================================================================
// DARK MODE DETECTION
// =============================================================================

fn detect_dark_mode() -> bool {
    if let Ok(v) = std::env::var("ABBOT_TUI_THEME") {
        match v.trim().to_ascii_lowercase().as_str() {
            "dark" => return true,
            "light" => return false,
            _ => {}
        }
    }

    if let Ok(v) = std::env::var("COLORFGBG")
        && let Some(bg) = v
            .split(';')
            .filter_map(|p| p.parse::<u8>().ok())
            .next_back()
    {
        return bg <= 6;
    }

    true
}

// =============================================================================
// EVENT LOOP
// =============================================================================

async fn run_app(
    addr: String,
    room: String,
    replay_live: Option<u64>,
    developer: bool,
) -> io::Result<()> {
    let dark_mode = detect_dark_mode();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(&room, dark_mode, developer);

    // WebSocket channels
    let (event_tx, mut event_rx) = mpsc::channel::<WsEvent>(256);
    let (cmd_tx, cmd_rx) = mpsc::channel::<WsInMessage>(64);
    let replay_tx = event_tx.clone();

    // Spawn background task: live replay or WebSocket
    if let Some(since_seq) = replay_live {
        let replay_addr = addr.clone();
        let replay_event_tx = event_tx.clone();
        tokio::spawn(async move {
            replay_live::run_replay(replay_addr, since_seq, replay_event_tx).await;
        });
        // Drop cmd_rx so it doesn't block (no WS to consume commands).
        drop(cmd_rx);
    } else {
        let ws_addr = addr.clone();
        tokio::spawn(async move {
            ws::run_ws(ws_addr, event_tx, cmd_rx).await;
        });
    }

    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // Global: Ctrl-C quits
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        break;
                    }

                    match app.mode {
                        Mode::Normal => match key.code {
                            KeyCode::Char('i') => {
                                app.mode = Mode::Insert;
                            }
                            KeyCode::Char('q') => break,
                            KeyCode::Char('1') => {
                                app.active_view = AppView::Chat;
                                app.active_room = 0;
                                app.rooms[0].unread = false;
                            }
                            KeyCode::Char('2') => {
                                app.active_view = AppView::Hands;
                            }
                            KeyCode::Char('3') if app.developer => {
                                app.active_view = AppView::Frames;
                            }
                            KeyCode::Char('4') if app.developer => {
                                app.active_view = AppView::Ems;
                            }
                            KeyCode::Tab => {
                                let next = (app.active_room + 1) % app.rooms.len();
                                app.active_room = next;
                                app.rooms[next].unread = false;
                                app.active_view = AppView::Chat;
                            }
                            KeyCode::BackTab => {
                                let prev = if app.active_room == 0 {
                                    app.rooms.len() - 1
                                } else {
                                    app.active_room - 1
                                };
                                app.active_room = prev;
                                app.rooms[prev].unread = false;
                                app.active_view = AppView::Chat;
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                let room = app.current_room_mut();
                                room.scroll_offset = room.scroll_offset.saturating_sub(1);
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                app.current_room_mut().scroll_offset += 1;
                            }
                            KeyCode::Char('G') => {
                                app.current_room_mut().scroll_offset = 0;
                            }
                            KeyCode::Char('a') => {
                                app.show_activity = !app.show_activity;
                            }
                            _ => {}
                        },
                        Mode::Insert if app.input_pending => {
                            // Input locked — only allow Esc while waiting.
                            if key.code == KeyCode::Esc {
                                app.mode = Mode::Normal;
                            }
                        }
                        Mode::Insert => match key.code {
                            KeyCode::Esc => {
                                app.mode = Mode::Normal;
                            }
                            KeyCode::Enter => {
                                let text = app.input.value().to_string();
                                if text.starts_with('/') {
                                    // Slash command dispatch
                                    let parts: Vec<&str> =
                                        text.splitn(2, char::is_whitespace).collect();
                                    let cmd = parts[0];
                                    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");
                                    match cmd {
                                        "/join" if !arg.is_empty() => {
                                            let idx = app.ensure_room(arg);
                                            app.active_room = idx;
                                            app.rooms[idx].unread = false;
                                        }
                                        "/part" | "/close" => {
                                            if app.rooms.len() > 1 {
                                                app.rooms.remove(app.active_room);
                                                if app.active_room >= app.rooms.len() {
                                                    app.active_room = app.rooms.len() - 1;
                                                }
                                            }
                                        }
                                        "/clear" => {
                                            let room = app.current_room_mut();
                                            room.messages.clear();
                                            room.streaming_buf.clear();
                                            room.pending_activity.clear();
                                            room.pending_seq = None;
                                        }
                                        "/cancel" => {
                                            let room = app.current_room_mut();
                                            room.pending = false;
                                            room.streaming_buf.clear();
                                            room.pending_activity.clear();
                                            room.pending_seq = None;
                                            let room_name = room.room.clone();
                                            let frame = ws::Frame::req(
                                                "chat:cancel",
                                                serde_json::json!({
                                                    "room": &room_name,
                                                    "reason": "client_cancel",
                                                }),
                                            )
                                            .with_actor("system");
                                            if cmd_tx
                                                .try_send(WsInMessage::FrameMsg { frame })
                                                .is_err()
                                            {
                                                app.current_room_mut().messages.push(ChatEntry {
                                                    timestamp: chrono::Local::now(),
                                                    kind: EntryKind::System,
                                                    content: "Cancel failed: outbound queue full"
                                                        .into(),
                                                    status: app::MessageStatus::None,
                                                    activity: Vec::new(),
                                                    seq: None,
                                                });
                                            }
                                        }
                                        "/quit" | "/q" => break,
                                        _ => {
                                            let room = app.current_room_mut();
                                            room.messages.push(ChatEntry {
                                                timestamp: chrono::Local::now(),
                                                kind: EntryKind::System,
                                                content: format!("Unknown command: {}", cmd),
                                                status: app::MessageStatus::None,
                                                activity: Vec::new(),
                                                seq: None,
                                            });
                                        }
                                    }
                                    app.input.reset();
                                } else if let Some(rest) = text.strip_prefix('!') {
                                    // Bash escape mode
                                    let shell_cmd = rest.trim().to_string();
                                    app.current_room_mut().messages.push(ChatEntry {
                                        timestamp: chrono::Local::now(),
                                        kind: EntryKind::User,
                                        content: format!("! {}", shell_cmd),
                                        status: app::MessageStatus::None,
                                        activity: Vec::new(),
                                        seq: None,
                                    });
                                    app.input.reset();

                                    if shell_cmd.is_empty() {
                                        app.current_room_mut().messages.push(ChatEntry {
                                            timestamp: chrono::Local::now(),
                                            kind: EntryKind::System,
                                            content: "No command given".into(),
                                            status: app::MessageStatus::None,
                                            activity: Vec::new(),
                                            seq: None,
                                        });
                                    } else {
                                        let output = tokio::process::Command::new("sh")
                                            .arg("-c")
                                            .arg(&shell_cmd)
                                            .output()
                                            .await;
                                        let content = match output {
                                            Ok(out) => {
                                                let combined = format!(
                                                    "{}{}",
                                                    String::from_utf8_lossy(&out.stdout),
                                                    String::from_utf8_lossy(&out.stderr),
                                                );
                                                let trimmed = combined.trim();
                                                if trimmed.is_empty() {
                                                    "(no output)".into()
                                                } else {
                                                    trimmed.to_string()
                                                }
                                            }
                                            Err(e) => format!("Error: {}", e),
                                        };
                                        app.current_room_mut().messages.push(ChatEntry {
                                            timestamp: chrono::Local::now(),
                                            kind: EntryKind::System,
                                            content,
                                            status: app::MessageStatus::None,
                                            activity: Vec::new(),
                                            seq: None,
                                        });
                                    }
                                    app.current_room_mut().scroll_offset = 0;
                                } else if !text.is_empty() {
                                    let room_name = app.current_room().room.clone();
                                    let frame = ws::Frame::req(
                                        "chat:message",
                                        serde_json::json!({
                                            "room": &room_name,
                                            "content": &text,
                                            "interactive": true,
                                        }),
                                    )
                                    .with_actor("user");
                                    if cmd_tx.try_send(WsInMessage::FrameMsg { frame }).is_err() {
                                        app.current_room_mut().messages.push(ChatEntry {
                                            timestamp: chrono::Local::now(),
                                            kind: EntryKind::System,
                                            content: "Send failed: outbound queue full".into(),
                                            status: app::MessageStatus::None,
                                            activity: Vec::new(),
                                            seq: None,
                                        });
                                    } else {
                                        // Keep text in input box (disabled)
                                        // until server echoes it back via ChatAck.
                                        app.input_pending = true;
                                        app.input_pending_room = room_name;
                                    }
                                } else {
                                    app.input.reset();
                                }
                            }
                            KeyCode::Char(c) => {
                                app.input.handle(tui_input::InputRequest::InsertChar(c));
                            }
                            KeyCode::Backspace => {
                                app.input.handle(tui_input::InputRequest::DeletePrevChar);
                            }
                            KeyCode::Delete => {
                                app.input.handle(tui_input::InputRequest::DeleteNextChar);
                            }
                            KeyCode::Left => {
                                app.input.handle(tui_input::InputRequest::GoToPrevChar);
                            }
                            KeyCode::Right => {
                                app.input.handle(tui_input::InputRequest::GoToNextChar);
                            }
                            KeyCode::Home => {
                                app.input.handle(tui_input::InputRequest::GoToStart);
                            }
                            KeyCode::End => {
                                app.input.handle(tui_input::InputRequest::GoToEnd);
                            }
                            _ => {}
                        },
                    }
                }
                _ => {}
            }
        }

        // Drain WebSocket events
        if last_tick.elapsed() >= tick_rate {
            while let Ok(event) = event_rx.try_recv() {
                match event {
                    WsEvent::Connected => {
                        app.connected = true;
                        // Register compiled-in tools for the initial room.
                        {
                            let reg_frame = tools::registration_frame(&room);
                            let _ = cmd_tx.try_send(WsInMessage::FrameMsg { frame: reg_frame });
                        }
                        // Request a dynamic farewell message (fire-and-forget).
                        {
                            let frame = ws::Frame::req(
                                "chat:llm",
                                serde_json::json!({
                                    "messages": [
                                        {
                                            "role": "system",
                                            "content": "You write single-line zen farewell messages for a CLI tool called Abbot \
                                                (an octopus monk who codes). The tone is calm, wise, slightly whimsical. \
                                                Themes: coding, monasteries, tentacles, git, deploys, rest. \
                                                Respond with exactly one short sentence — nothing else."
                                        },
                                        { "role": "user", "content": "Write a farewell." }
                                    ]
                                }),
                            )
                            .with_actor("hand/farewell");
                            app.farewell_req_id = Some(frame.id);
                            app.farewell_buf.clear();
                            let _ = cmd_tx.try_send(WsInMessage::FrameMsg { frame });
                        }
                        // Spawn replay fetches for every known room.
                        for r in &app.rooms {
                            let tx = replay_tx.clone();
                            let a = addr.clone();
                            let room_name = r.room.clone();
                            let since = r.last_replay_ts;
                            tokio::spawn(async move {
                                let (events, max_ts) =
                                    replay::fetch_history(&a, &room_name, since).await;
                                for event in events {
                                    if tx.send(event).await.is_err() {
                                        return;
                                    }
                                }
                                if max_ts > since {
                                    let _ = tx
                                        .send(WsEvent::ReplaySync {
                                            room: room_name,
                                            max_ts,
                                        })
                                        .await;
                                }
                            });
                        }

                        // Resend pending input if we disconnected mid-send.
                        if app.input_pending {
                            let frame = ws::Frame::req(
                                "chat:message",
                                serde_json::json!({
                                    "room": &app.input_pending_room,
                                    "content": app.input.value(),
                                    "interactive": true,
                                }),
                            )
                            .with_actor("user");
                            let _ = cmd_tx.try_send(WsInMessage::FrameMsg { frame });
                        }
                    }
                    WsEvent::Disconnected => {
                        app.connected = false;
                        app.farewell_req_id = None;
                        app.farewell_buf.clear();
                        // Flush partial streaming buffers so text isn't lost.
                        for room in &mut app.rooms {
                            room.flush_stream();
                            room.pending_seq = None;
                        }
                    }
                    WsEvent::ChatAck { room, thread_id } => {
                        let idx = app.ensure_room(&room);
                        // Track thread_id for tool call ownership routing.
                        if !thread_id.is_empty() {
                            app.rooms[idx].thread_id = Some(thread_id);
                        }

                        // Server acknowledged — move text from input into chat.
                        if app.input_pending && app.input_pending_room == room {
                            let text = app.input.value().to_string();
                            app.input.reset();
                            app.input_pending = false;
                            app.input_pending_room.clear();

                            app.rooms[idx].messages.push(ChatEntry {
                                timestamp: chrono::Local::now(),
                                kind: EntryKind::User,
                                content: text,
                                status: app::MessageStatus::Sent,
                                activity: Vec::new(),
                                seq: None,
                            });
                            app.rooms[idx].pending = true;
                            app.rooms[idx].scroll_offset = 0;
                        }
                    }
                    WsEvent::ChatDelta { room, content, seq } => {
                        let idx = app.ensure_room(&room);
                        app.rooms[idx].pending = true;
                        app.rooms[idx].streaming_buf.push_str(&content);
                        if app.rooms[idx].pending_seq.is_none() {
                            app.rooms[idx].pending_seq = seq;
                        }
                        app.rooms[idx].status_text = None;
                        if idx != app.active_room {
                            app.rooms[idx].unread = true;
                        }
                    }
                    WsEvent::ChatTool {
                        room,
                        reply_to,
                        tool_call_id,
                        name,
                        arguments,
                    } => {
                        let idx = app.ensure_room(&room);

                        // Only execute if this tool call belongs to our turn.
                        let is_ours = app
                            .rooms
                            .get(idx)
                            .and_then(|r| r.thread_id.as_ref())
                            .is_some_and(|tid| tid == &reply_to);

                        // Display tool call with in-progress icon.
                        let line = if is_ours {
                            let summary = tools::tool_summary(&name, &arguments);
                            format!("~ tui $ {} {}", name, summary)
                                .trim_end()
                                .to_string()
                        } else {
                            format!("~ {}", name)
                        };
                        if app.rooms[idx].pending
                            || !app.rooms[idx].streaming_buf.is_empty()
                            || app.rooms[idx].status_text.is_some()
                        {
                            app.rooms[idx].pending_activity.push(line);
                        } else {
                            app.rooms[idx].messages.push(ChatEntry {
                                timestamp: chrono::Local::now(),
                                kind: EntryKind::Activity,
                                content: line,
                                status: app::MessageStatus::None,
                                activity: Vec::new(),
                                seq: None,
                            });
                        }

                        if is_ours {
                            let cmd_tx2 = cmd_tx.clone();
                            let event_tx2 = replay_tx.clone();
                            let room2 = room.clone();
                            let reply_to2 = reply_to.clone();
                            let tcid = tool_call_id.clone();
                            let name2 = name.clone();
                            tokio::spawn(async move {
                                let result = tools::execute_tool(&name2, &arguments).await;
                                let _ = event_tx2
                                    .send(WsEvent::ToolExecDone {
                                        room: room2.clone(),
                                        name: name2.clone(),
                                        is_error: result.is_error,
                                    })
                                    .await;
                                // Send result to daemon.
                                let frame = ws::Frame::req(
                                    "chat:tool_result",
                                    serde_json::json!({
                                        "room": room2,
                                        "reply_to": reply_to2,
                                        "tool_call_id": tcid,
                                        "name": name2,
                                        "content": result.content,
                                        "is_error": result.is_error,
                                    }),
                                )
                                .with_actor("user");
                                let _ = cmd_tx2.try_send(WsInMessage::FrameMsg { frame });
                            });
                        }
                    }
                    WsEvent::ToolExecDone {
                        room,
                        name,
                        is_error,
                    } => {
                        let idx = app.ensure_room(&room);
                        let icon = if is_error { "\u{2717}" } else { "\u{2713}" };
                        let search = format!("~ tui $ {}", name);
                        update_tool_icon(&mut app.rooms[idx], &search, icon);
                    }
                    WsEvent::ChatDone { room } => {
                        let idx = app.ensure_room(&room);
                        // Mark remaining in-progress tool icons as done.
                        resolve_pending_icons(&mut app.rooms[idx]);
                        app.rooms[idx].flush_stream();
                        app.rooms[idx].pending = false;
                        app.rooms[idx].status_text = None;
                        app.rooms[idx].pending_seq = None;
                        if idx != app.active_room {
                            app.rooms[idx].unread = true;
                        }
                    }
                    WsEvent::ChatError { room, message } => {
                        let idx = app.ensure_room(&room);
                        app.rooms[idx].flush_stream();
                        app.rooms[idx].pending = false;
                        app.rooms[idx].status_text = None;
                        app.rooms[idx].pending_seq = None;
                        app.rooms[idx].messages.push(ChatEntry {
                            timestamp: chrono::Local::now(),
                            kind: EntryKind::System,
                            content: format!("Error: {}", message),
                            status: app::MessageStatus::None,
                            activity: Vec::new(),
                            seq: None,
                        });
                    }
                    WsEvent::ChatStatus {
                        room,
                        status,
                        actor,
                        tool,
                        summary,
                        content,
                    } => {
                        let idx = app.ensure_room(&room);
                        if status == "thinking" {
                            app.rooms[idx].status_text = Some("[thinking..]".to_string());
                        } else if status == "thought" {
                            // Intermediate LLM text emitted before tool calls —
                            // push into pending_activity so it's interleaved
                            // chronologically with tool activity lines.
                            if let Some(text) = content {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    if app.rooms[idx].pending
                                        || !app.rooms[idx].streaming_buf.is_empty()
                                        || app.rooms[idx].status_text.is_some()
                                    {
                                        app.rooms[idx].pending_activity.push(trimmed.to_string());
                                    } else {
                                        app.rooms[idx].messages.push(ChatEntry {
                                            timestamp: chrono::Local::now(),
                                            kind: EntryKind::Activity,
                                            content: trimmed.to_string(),
                                            status: app::MessageStatus::None,
                                            activity: Vec::new(),
                                            seq: None,
                                        });
                                    }
                                    app.rooms[idx].status_text = None;
                                }
                            }
                        } else if status == "tool" {
                            let actor_name = actor.as_deref().unwrap_or("");
                            let tool_name = tool.as_deref().unwrap_or("");
                            let tool_summary = summary.as_deref().unwrap_or("");
                            let line = format!("~ {} $ {} {}", actor_name, tool_name, tool_summary)
                                .trim_end()
                                .to_string();
                            if app.rooms[idx].pending
                                || !app.rooms[idx].streaming_buf.is_empty()
                                || app.rooms[idx].status_text.is_some()
                            {
                                app.rooms[idx].pending_activity.push(line);
                            } else {
                                app.rooms[idx].messages.push(ChatEntry {
                                    timestamp: chrono::Local::now(),
                                    kind: EntryKind::Activity,
                                    content: line,
                                    status: app::MessageStatus::None,
                                    activity: Vec::new(),
                                    seq: None,
                                });
                            }
                            app.rooms[idx].status_text = None;
                        }
                    }
                    WsEvent::HandStart {
                        room: _,
                        actor,
                        tool,
                        summary,
                    } => {
                        app.hand_log.push(app::HandLogEntry {
                            timestamp: chrono::Local::now(),
                            actor,
                            tool,
                            summary,
                        });
                    }
                    WsEvent::HandEnd { .. } => {}
                    WsEvent::ChatMind { room, content } => {
                        let idx = app.ensure_room(&room);
                        app.rooms[idx].flush_stream();
                        app.rooms[idx].messages.push(ChatEntry {
                            timestamp: chrono::Local::now(),
                            kind: EntryKind::Mind,
                            content,
                            status: app::MessageStatus::None,
                            activity: Vec::new(),
                            seq: None,
                        });
                        if idx != app.active_room {
                            app.rooms[idx].unread = true;
                        }
                    }
                    WsEvent::Frame(frame) => {
                        // Farewell accumulation: collect text_delta from farewell LLM call.
                        if let Some(ref farewell_id) = app.farewell_req_id
                            && frame.parent_id.as_ref() == Some(farewell_id)
                        {
                            match frame.op {
                                ws::FrameOp::Item => {
                                    if let Some(data) = &frame.data
                                        && data.get("type").and_then(|v| v.as_str())
                                            == Some("text_delta")
                                        && let Some(text) =
                                            data.get("content").and_then(|v| v.as_str())
                                    {
                                        app.farewell_buf.push_str(text);
                                    }
                                }
                                ws::FrameOp::Done => {
                                    let text = app.farewell_buf.trim().to_string();
                                    if !text.is_empty() {
                                        app.farewell_text = Some(text);
                                    }
                                    app.farewell_req_id = None;
                                    app.farewell_buf.clear();
                                }
                                ws::FrameOp::Error => {
                                    app.farewell_req_id = None;
                                    app.farewell_buf.clear();
                                }
                                _ => {}
                            }
                        }

                        // Extract daemon seq from SIGTICK, filter it from ticker.
                        let is_sigtick =
                            frame.data.as_ref().and_then(|d| d["kind"].as_str()) == Some("SIGTICK");
                        if is_sigtick {
                            if let Some(seq) = frame.data.as_ref().and_then(|d| d["seq"].as_u64()) {
                                app.daemon_seq = seq;
                            }
                        } else {
                            // Populate frame ticker.
                            app.ticker_seq += 1;
                            app.ticker
                                .push_back(format_frame_line(app.ticker_seq, &frame));
                            if app.ticker.len() > 64 {
                                app.ticker.pop_front();
                            }
                        }
                    }
                    WsEvent::ReplayUser { room, content, seq } => {
                        let idx = app.ensure_room(&room);
                        app.rooms[idx].messages.push(ChatEntry {
                            timestamp: chrono::Local::now(),
                            kind: EntryKind::User,
                            content,
                            status: app::MessageStatus::Sent,
                            activity: Vec::new(),
                            seq: Some(seq),
                        });
                        app.rooms[idx].scroll_offset = 0;
                    }
                    WsEvent::ReplaySync { room, max_ts } => {
                        let idx = app.ensure_room(&room);
                        // Flush any partial streaming buffer from replayed deltas.
                        app.rooms[idx].flush_stream();
                        app.rooms[idx].last_replay_ts = max_ts;
                    }
                }
            }

            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    print_farewell(app.farewell_text.as_deref());
    Ok(())
}

fn print_farewell(dynamic: Option<&str>) {
    const BLUE: &str = "\x1b[34m";
    const WHITE: &str = "\x1b[97m";
    const DIM: &str = "\x1b[2m";
    const RESET: &str = "\x1b[0m";

    let line = if let Some(text) = dynamic {
        text.to_string()
    } else {
        let farewells: Vec<&str> = include_str!("farewells.txt")
            .lines()
            .filter(|l| !l.is_empty())
            .collect();
        if farewells.is_empty() {
            println!();
            println!("  {BLUE}▗▄███▄▖{RESET}");
            println!("  {BLUE} █{RESET}◉ ◉{BLUE}█{RESET}");
            println!("  {BLUE} ⠿ ⠿ ⠿{RESET}");
            println!();
            return;
        }
        let index = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as usize % farewells.len())
            .unwrap_or(0);
        farewells[index].to_string()
    };

    println!();
    println!("  {BLUE}▗▄███▄▖{RESET}");
    println!("  {BLUE} █{WHITE}◉ ◉{BLUE}█{RESET}");
    println!("  {BLUE} ⠿ ⠿ ⠿{RESET}");
    println!();
    println!("  {DIM}{line}{RESET}");
    println!();
}

// =============================================================================
// TOOL ICON HELPERS
// =============================================================================

/// Update a `~ ` prefixed tool line to a completion icon (✓ or ✗).
/// Searches pending_activity then committed message activity in reverse.
fn update_tool_icon(room: &mut app::Room, search_prefix: &str, icon: &str) {
    // Search pending_activity (most common — tool completes while still streaming)
    for line in room.pending_activity.iter_mut().rev() {
        if line.starts_with(search_prefix) {
            line.replace_range(..1, icon);
            return;
        }
    }
    // Search committed messages (tool completed after flush)
    for entry in room.messages.iter_mut().rev() {
        for line in entry.activity.iter_mut().rev() {
            if line.starts_with(search_prefix) {
                line.replace_range(..1, icon);
                return;
            }
        }
        if entry.kind == EntryKind::Activity && entry.content.starts_with(search_prefix) {
            entry.content.replace_range(..1, icon);
            return;
        }
    }
}

/// Mark all remaining `~ ` tool lines as ✓ (called on ChatDone).
fn resolve_pending_icons(room: &mut app::Room) {
    for line in &mut room.pending_activity {
        if line.starts_with("~ ") {
            line.replace_range(..1, "\u{2713}");
        }
    }
}

// =============================================================================
// FRAME TICKER FORMATTING (mirrors CLI `frames transcript` output)
// =============================================================================

fn format_frame_line(seq: u64, frame: &ws::Frame) -> String {
    use chrono::{Local, TimeZone};

    let ts = Local.timestamp_millis_opt(frame.ts).single();
    let time_str = match ts {
        Some(t) => t.format("%H:%M").to_string(),
        None => "--:--".into(),
    };

    let id_str = frame.id.to_string();
    let short_id = &id_str[..8];
    let op = match frame.op {
        ws::FrameOp::Req => "req",
        ws::FrameOp::Ok => "ok",
        ws::FrameOp::Done => "done",
        ws::FrameOp::Error => "error",
        ws::FrameOp::Event => "event",
        ws::FrameOp::Cancel => "cancel",
        ws::FrameOp::Item => "item",
        ws::FrameOp::Bytes => "bytes",
        ws::FrameOp::Progress => "progress",
    };
    let name = frame.name.as_deref().unwrap_or("");
    let actor = frame.actor.as_deref().unwrap_or("");
    let data = frame.data.as_ref();
    let room = data.and_then(|d| d["room"].as_str()).unwrap_or("");
    let kind = data.and_then(|d| d["kind"].as_str()).unwrap_or("");

    let tail = ticker_content(op, name, kind, data);
    format!(
        "{time_str} seq={seq} id={short_id} op={op} name={name} actor={actor} room={room}{tail}"
    )
}

fn ticker_content(op: &str, name: &str, kind: &str, data: Option<&serde_json::Value>) -> String {
    match op {
        "req" => ticker_content_req(name, data),
        "ok" => ticker_content_ok(data),
        "done" => String::new(),
        "error" => {
            let code = data.and_then(|d| d["code"].as_str()).unwrap_or("UNKNOWN");
            let msg = data.and_then(|d| d["message"].as_str()).unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(msg, 120));
            format!(" code={code} content=\"{escaped}\"")
        }
        "item" => ticker_content_item(name, data),
        "event" => ticker_content_event(kind, data),
        "progress" | "bytes" => String::new(),
        _ => ticker_fallback(data),
    }
}

fn ticker_content_req(name: &str, data: Option<&serde_json::Value>) -> String {
    match name {
        "need:lease" | "task:lease" | "tick:subscribe" => String::new(),
        "chat:message" => {
            let content = data.and_then(|d| d["content"].as_str()).unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(content, 120));
            format!(" content=\"{escaped}\"")
        }
        "chat:llm" => {
            let n = data
                .and_then(|d| d["messages"].as_array())
                .map_or(0, |a| a.len());
            format!(" content=\"messages={n}\"")
        }
        "chat:status" => {
            let status = data.and_then(|d| d["status"].as_str()).unwrap_or("?");
            format!(" content=\"status={status}\"")
        }
        "chat:done" => {
            let reason = data
                .and_then(|d| d["reason"].as_str().or_else(|| d["stop_reason"].as_str()))
                .unwrap_or("");
            if reason.is_empty() {
                String::new()
            } else {
                format!(" content=\"reason={reason}\"")
            }
        }
        "chat:tool" => {
            let tool = data.and_then(|d| d["name"].as_str()).unwrap_or("?");
            let args = data
                .map(|d| {
                    if d["arguments"].is_string() {
                        d["arguments"].as_str().unwrap_or("").to_string()
                    } else if d["arguments"].is_object() {
                        serde_json::to_string(&d["arguments"]).unwrap_or_default()
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            let escaped = ticker_escape(&ticker_truncate(&args, 80));
            format!(" content=\"tool={tool} args={escaped}\"")
        }
        "chat:tool_result" => {
            let tool = data.and_then(|d| d["name"].as_str()).unwrap_or("?");
            let is_error = data.and_then(|d| d["is_error"].as_bool()).unwrap_or(false);
            let content = data.and_then(|d| d["content"].as_str()).unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(content, 80));
            format!(" content=\"tool={tool} is_error={is_error} {escaped}\"")
        }
        "chat:cancel" => {
            let reason = data.and_then(|d| d["reason"].as_str()).unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(reason, 120));
            format!(" content=\"reason={escaped}\"")
        }
        "need:enqueue" => {
            let priority = data.and_then(|d| d["priority"].as_str()).unwrap_or("?");
            let text = data
                .and_then(|d| d["need"].as_str().or_else(|| d["text"].as_str()))
                .unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(text, 100));
            format!(" content=\"priority={priority} {escaped}\"")
        }
        "need:fulfill" => {
            let id = data
                .and_then(|d| d["need_id"].as_str().or_else(|| d["id"].as_str()))
                .unwrap_or("?");
            let short = if id.len() > 8 { &id[..8] } else { id };
            let summary = data.and_then(|d| d["summary"].as_str()).unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(summary, 100));
            format!(" content=\"need_id={short} {escaped}\"")
        }
        "task:enqueue" => {
            let prompt = data.and_then(|d| d["prompt"].as_str()).unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(prompt, 120));
            format!(" content=\"{escaped}\"")
        }
        "task:complete" => {
            let ok = data.and_then(|d| d["ok"].as_bool()).unwrap_or(false);
            let summary = data.and_then(|d| d["summary"].as_str()).unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(summary, 100));
            format!(" content=\"ok={ok} {escaped}\"")
        }
        "tool:register" => {
            let n = data
                .and_then(|d| d["tools"].as_array())
                .map_or(0, |a| a.len());
            format!(" content=\"tools={n}\"")
        }
        _ if name.starts_with("ems:") => {
            let table = data.and_then(|d| d["table"].as_str()).unwrap_or("?");
            format!(" content=\"table={table}\"")
        }
        _ => ticker_fallback(data),
    }
}

fn ticker_content_ok(data: Option<&serde_json::Value>) -> String {
    if let Some(obj) = data.and_then(|d| d.as_object()) {
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

fn ticker_content_item(name: &str, data: Option<&serde_json::Value>) -> String {
    let item_type = data.and_then(|d| d["type"].as_str()).unwrap_or("");
    match (name, item_type) {
        ("chat:llm", "text_delta") | ("chat:llm", "thinking") => {
            let content = data.and_then(|d| d["content"].as_str()).unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(content, 120));
            format!(" content=\"{escaped}\"")
        }
        ("chat:llm", "tool_call") => {
            let tool = data.and_then(|d| d["name"].as_str()).unwrap_or("?");
            let args = data
                .map(|d| {
                    if d["arguments"].is_string() {
                        d["arguments"].as_str().unwrap_or("").to_string()
                    } else if d["arguments"].is_object() {
                        serde_json::to_string(&d["arguments"]).unwrap_or_default()
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            let escaped = ticker_escape(&ticker_truncate(&args, 80));
            format!(" content=\"tool={tool} {escaped}\"")
        }
        ("chat:llm", "tool_result") => {
            let tool = data.and_then(|d| d["name"].as_str()).unwrap_or("?");
            let content = data.and_then(|d| d["content"].as_str()).unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(content, 80));
            format!(" content=\"tool={tool} {escaped}\"")
        }
        ("chat:message", _) => {
            let content = data
                .and_then(|d| {
                    d["content"]
                        .as_str()
                        .or_else(|| d["data"]["content"].as_str())
                })
                .unwrap_or("");
            let escaped = ticker_escape(&ticker_truncate(content, 120));
            format!(" content=\"{escaped}\"")
        }
        ("chat:status", _) => {
            let status = data.and_then(|d| d["status"].as_str()).unwrap_or("?");
            format!(" content=\"status={status}\"")
        }
        ("chat:done", _) => {
            let reason = data.and_then(|d| d["reason"].as_str()).unwrap_or("?");
            format!(" content=\"reason={reason}\"")
        }
        ("chat:tool", _) => {
            let tool = data.and_then(|d| d["name"].as_str()).unwrap_or("?");
            format!(" content=\"tool={tool}\"")
        }
        _ => ticker_fallback(data),
    }
}

fn ticker_content_event(kind: &str, data: Option<&serde_json::Value>) -> String {
    match kind {
        "llm:begin" => {
            let model = data.and_then(|d| d["model"].as_str()).unwrap_or("?");
            let provider = data.and_then(|d| d["provider"].as_str()).unwrap_or("?");
            let msgs = data.and_then(|d| d["messages"].as_u64()).unwrap_or(0);
            let tools = data.and_then(|d| d["tools"].as_u64()).unwrap_or(0);
            format!(" content=\"model={model} provider={provider} msgs={msgs} tools={tools}\"")
        }
        "llm:result" => {
            let completion = data
                .and_then(|d| {
                    d["usage"]["completion_tokens"]
                        .as_u64()
                        .or_else(|| d["completion_tokens"].as_u64())
                })
                .unwrap_or(0);
            let prompt = data
                .and_then(|d| {
                    d["usage"]["prompt_tokens"]
                        .as_u64()
                        .or_else(|| d["prompt_tokens"].as_u64())
                })
                .unwrap_or(0);
            let total = completion + prompt;
            format!(" content=\"completion={completion} prompt={prompt} total={total}\"")
        }
        "chat.ack" => {
            let thread_id = data.and_then(|d| d["thread_id"].as_str()).unwrap_or("?");
            format!(" content=\"thread_id={thread_id}\"")
        }
        "hand:start" => {
            let tool = data
                .and_then(|d| d["tool"].as_str().or_else(|| d["name"].as_str()))
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

fn ticker_fallback(data: Option<&serde_json::Value>) -> String {
    let data = match data {
        Some(d) => d,
        None => return String::new(),
    };

    let text = data["content"]
        .as_str()
        .or_else(|| data["data"]["content"].as_str())
        .or_else(|| data["summary"].as_str())
        .or_else(|| data["message"].as_str())
        .or_else(|| data["prompt"].as_str())
        .or_else(|| data["reason"].as_str());

    if let Some(t) = text
        && !t.is_empty()
    {
        let escaped = ticker_escape(&ticker_truncate(t, 120));
        return format!(" content=\"{escaped}\"");
    }

    let compact = serde_json::to_string(data).unwrap_or_default();
    if compact != "null" && compact != "{}" {
        let escaped = ticker_escape(&ticker_truncate(&compact, 120));
        format!(" content=\"{escaped}\"")
    } else {
        String::new()
    }
}

fn ticker_truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}...")
}

fn ticker_escape(s: &str) -> String {
    s.trim()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

// =============================================================================
// ENTRY POINT
// =============================================================================

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();
    run_app(cli.addr, cli.room, cli.replay_live, cli.developer).await
}
