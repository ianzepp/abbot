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
mod ui;
mod ws;

use std::io;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
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

async fn run_app(addr: String, room: String, replay_live: Option<u64>) -> io::Result<()> {
    let dark_mode = detect_dark_mode();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(&room, dark_mode);

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
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        app.current_room_mut().scroll_offset += 3;
                    }
                    MouseEventKind::ScrollDown => {
                        let room = app.current_room_mut();
                        room.scroll_offset = room.scroll_offset.saturating_sub(3);
                    }
                    _ => {}
                },
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
                                app.active_view = AppView::Frames;
                            }
                            KeyCode::Char('3') => {
                                app.active_view = AppView::Hands;
                            }
                            KeyCode::Char('4') => {
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
                    WsEvent::ChatAck { room } => {
                        let idx = app.ensure_room(&room);

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
                    WsEvent::ChatTool { room, name } => {
                        let idx = app.ensure_room(&room);
                        let line = format!("tool: {}", name);
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
                    }
                    WsEvent::ChatDone { room } => {
                        let idx = app.ensure_room(&room);
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
                            let line = format!("{} $ {} {}", actor_name, tool_name, tool_summary)
                                .trim()
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
    execute!(
        terminal.backend_mut(),
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen
    )?;
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
// ENTRY POINT
// =============================================================================

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();
    run_app(cli.addr, cli.room, cli.replay_live).await
}
