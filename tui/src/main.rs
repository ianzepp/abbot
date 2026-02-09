//! abbot-tui — multi-room chat TUI for the Abbot daemon.
//!
//! Connects to the daemon via WebSocket for real-time chat with streaming
//! responses, tool call visibility, and multi-room chat tabs.

mod app;
mod markdown;
mod replay;
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

use app::{App, ChatEntry, EntryKind, Mode};
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

async fn run_app(addr: String, room: String) -> io::Result<()> {
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

    // Spawn WebSocket background task
    let ws_addr = addr.clone();
    tokio::spawn(async move {
        ws::run_ws(ws_addr, event_tx, cmd_rx).await;
    });

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
                            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                                let idx = (c as usize) - ('1' as usize);
                                if idx < app.rooms.len() {
                                    app.active_room = idx;
                                    app.rooms[idx].unread = false;
                                }
                            }
                            KeyCode::Tab => {
                                let next = (app.active_room + 1) % app.rooms.len();
                                app.active_room = next;
                                app.rooms[next].unread = false;
                            }
                            KeyCode::BackTab => {
                                let prev = if app.active_room == 0 {
                                    app.rooms.len() - 1
                                } else {
                                    app.active_room - 1
                                };
                                app.active_room = prev;
                                app.rooms[prev].unread = false;
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
                                        }
                                        "/cancel" => {
                                            let room = app.current_room_mut();
                                            room.pending = false;
                                            room.streaming_buf.clear();
                                            let room_name = room.room.clone();
                                            if cmd_tx
                                                .try_send(WsInMessage::ChatCancel {
                                                    room: room_name,
                                                })
                                                .is_err()
                                            {
                                                room.messages.push(ChatEntry {
                                                    timestamp: chrono::Local::now(),
                                                    kind: EntryKind::System,
                                                    content: "Cancel failed: outbound queue full"
                                                        .into(),
                                                    status: app::MessageStatus::None,
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
                                    });
                                    app.input.reset();

                                    if shell_cmd.is_empty() {
                                        app.current_room_mut().messages.push(ChatEntry {
                                            timestamp: chrono::Local::now(),
                                            kind: EntryKind::System,
                                            content: "No command given".into(),
                                            status: app::MessageStatus::None,
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
                                        });
                                    }
                                    app.current_room_mut().scroll_offset = 0;
                                } else if !text.is_empty() {
                                    let room = app.current_room_mut();
                                    room.messages.push(ChatEntry {
                                        timestamp: chrono::Local::now(),
                                        kind: EntryKind::User,
                                        content: text.clone(),
                                        status: app::MessageStatus::Pending,
                                    });
                                    room.pending = true;
                                    room.scroll_offset = 0;

                                    let room_name = room.room.clone();
                                    if cmd_tx
                                        .try_send(WsInMessage::ChatSend {
                                            room: room_name,
                                            text,
                                            id: None,
                                        })
                                        .is_err()
                                    {
                                        room.pending = false;
                                        room.messages.push(ChatEntry {
                                            timestamp: chrono::Local::now(),
                                            kind: EntryKind::System,
                                            content: "Send failed: outbound queue full".into(),
                                            status: app::MessageStatus::None,
                                        });
                                    }
                                    app.input.reset();
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
                        let _ = cmd_tx.try_send(WsInMessage::FarewellRequest);
                        // Spawn replay fetches for every known room.
                        for r in &app.rooms {
                            let tx = replay_tx.clone();
                            let a = addr.clone();
                            let room_name = r.room.clone();
                            let since = r.last_replay_ts;
                            tokio::spawn(async move {
                                let entries = replay::fetch_history(&a, &room_name, since).await;
                                if !entries.is_empty() {
                                    let _ = tx
                                        .send(WsEvent::ChatReplay {
                                            room: room_name,
                                            entries,
                                        })
                                        .await;
                                }
                            });
                        }

                        // Resend all pending messages across all rooms
                        for r in &app.rooms {
                            for (_idx, msg) in r.pending_messages() {
                                let _ = cmd_tx.try_send(WsInMessage::ChatSend {
                                    room: r.room.clone(),
                                    text: msg.content.clone(),
                                    id: None,
                                });
                            }
                        }
                    }
                    WsEvent::Disconnected => {
                        app.connected = false;
                        // Flush partial streaming buffers so text isn't lost.
                        for room in &mut app.rooms {
                            room.flush_stream();
                        }
                    }
                    WsEvent::ChatAck { room } => {
                        let idx = app.ensure_room(&room);

                        // Mark the most recent pending user message as sent
                        if let Some(pos) = app.rooms[idx].messages.iter().rposition(|m| {
                            m.kind == EntryKind::User && m.status == app::MessageStatus::Pending
                        }) {
                            app.rooms[idx].mark_sent(pos);
                        }
                    }
                    WsEvent::ChatDelta { room, content } => {
                        let idx = app.ensure_room(&room);
                        app.rooms[idx].pending = true;
                        app.rooms[idx].streaming_buf.push_str(&content);
                        app.rooms[idx].status_text = None;
                        if idx != app.active_room {
                            app.rooms[idx].unread = true;
                        }
                    }
                    WsEvent::ChatTool { room, name } => {
                        let idx = app.ensure_room(&room);
                        app.rooms[idx].flush_stream();
                        app.rooms[idx].messages.push(ChatEntry {
                            timestamp: chrono::Local::now(),
                            kind: EntryKind::Activity,
                            content: format!("tool: {}", name),
                            status: app::MessageStatus::None,
                        });
                    }
                    WsEvent::ChatDone { room } => {
                        let idx = app.ensure_room(&room);
                        app.rooms[idx].flush_stream();
                        app.rooms[idx].pending = false;
                        app.rooms[idx].status_text = None;
                        if idx != app.active_room {
                            app.rooms[idx].unread = true;
                        }
                    }
                    WsEvent::ChatError { room, message } => {
                        let idx = app.ensure_room(&room);
                        app.rooms[idx].flush_stream();
                        app.rooms[idx].pending = false;
                        app.rooms[idx].status_text = None;
                        app.rooms[idx].messages.push(ChatEntry {
                            timestamp: chrono::Local::now(),
                            kind: EntryKind::System,
                            content: format!("Error: {}", message),
                            status: app::MessageStatus::None,
                        });
                    }
                    WsEvent::ChatStatus {
                        room,
                        status,
                        actor,
                        tool,
                        summary,
                    } => {
                        let idx = app.ensure_room(&room);
                        if status == "thinking" {
                            app.rooms[idx].status_text = Some("[thinking..]".to_string());
                        } else if status == "tool" {
                            app.rooms[idx].flush_stream();
                            let actor_name = actor.as_deref().unwrap_or("");
                            let tool_name = tool.as_deref().unwrap_or("");
                            let tool_summary = summary.as_deref().unwrap_or("");
                            let line = format!("{} $ {} {}", actor_name, tool_name, tool_summary)
                                .trim()
                                .to_string();
                            app.rooms[idx].messages.push(ChatEntry {
                                timestamp: chrono::Local::now(),
                                kind: EntryKind::Activity,
                                content: line,
                                status: app::MessageStatus::None,
                            });
                            app.rooms[idx].status_text = None;
                        }
                    }
                    WsEvent::Farewell { text } => {
                        app.farewell_text = Some(text);
                    }
                    WsEvent::Frame(_frame) => {
                        // Background frame broadcast — could show as activity
                        // in matching rooms (future enhancement).
                    }
                    WsEvent::ChatReplay { room, entries } => {
                        let idx = app.ensure_room(&room);
                        let mut max_ts = app.rooms[idx].last_replay_ts;
                        for entry in entries {
                            let (kind, status) = match entry.kind {
                                replay::ReplayKind::User => {
                                    (EntryKind::User, app::MessageStatus::Sent)
                                }
                                replay::ReplayKind::Assistant => {
                                    (EntryKind::Assistant, app::MessageStatus::None)
                                }
                            };
                            if entry.ts_ms > max_ts {
                                max_ts = entry.ts_ms;
                            }
                            app.rooms[idx].messages.push(ChatEntry {
                                timestamp: chrono::Local::now(),
                                kind,
                                content: entry.content,
                                status,
                            });
                        }
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
    run_app(cli.addr, cli.room).await
}
