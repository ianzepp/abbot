//! abbot-tui — multi-room chat TUI for the Abbot daemon.
//!
//! Connects to the daemon via WebSocket for real-time chat with streaming
//! responses, tool call visibility, and multi-scope room tabs.

mod app;
mod room;
mod theme;
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

    /// Initial chat scope
    #[arg(long, default_value = "main")]
    scope: String,
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

async fn run_app(addr: String, scope: String) -> io::Result<()> {
    let dark_mode = detect_dark_mode();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(&scope, dark_mode);

    // WebSocket channels
    let (event_tx, mut event_rx) = mpsc::channel::<WsEvent>(256);
    let (cmd_tx, cmd_rx) = mpsc::channel::<WsInMessage>(64);

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
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            // Global: Ctrl-C quits
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
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
                        if !text.is_empty() {
                            let room = app.current_room_mut();
                            room.messages.push(ChatEntry {
                                timestamp: chrono::Local::now(),
                                kind: EntryKind::User,
                                content: text.clone(),
                            });
                            room.pending = true;
                            room.scroll_offset = 0;

                            let scope = room.scope.clone();
                            let _ = cmd_tx.try_send(WsInMessage::ChatSend {
                                scope,
                                text,
                                id: None,
                            });
                        }
                        app.input.reset();
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

        // Drain WebSocket events
        if last_tick.elapsed() >= tick_rate {
            while let Ok(event) = event_rx.try_recv() {
                match event {
                    WsEvent::Connected => app.connected = true,
                    WsEvent::Disconnected => app.connected = false,
                    WsEvent::ChatAck { scope } => {
                        let idx = app.ensure_room(&scope);
                        app.rooms[idx].pending = true;
                    }
                    WsEvent::ChatDelta { scope, content } => {
                        let idx = app.ensure_room(&scope);
                        app.rooms[idx].streaming_buf.push_str(&content);
                        if idx != app.active_room {
                            app.rooms[idx].unread = true;
                        }
                    }
                    WsEvent::ChatTool { scope, name } => {
                        let idx = app.ensure_room(&scope);
                        app.rooms[idx].messages.push(ChatEntry {
                            timestamp: chrono::Local::now(),
                            kind: EntryKind::Activity,
                            content: format!("tool: {}", name),
                        });
                    }
                    WsEvent::ChatDone { scope } => {
                        let idx = app.ensure_room(&scope);
                        app.rooms[idx].flush_stream();
                        app.rooms[idx].pending = false;
                        if idx != app.active_room {
                            app.rooms[idx].unread = true;
                        }
                    }
                    WsEvent::ChatError { scope, message } => {
                        let idx = app.ensure_room(&scope);
                        app.rooms[idx].flush_stream();
                        app.rooms[idx].pending = false;
                        app.rooms[idx].messages.push(ChatEntry {
                            timestamp: chrono::Local::now(),
                            kind: EntryKind::System,
                            content: format!("Error: {}", message),
                        });
                    }
                    WsEvent::Frame(_frame) => {
                        // Background frame broadcast — could show as activity
                        // in matching scope rooms (future enhancement).
                    }
                }
            }

            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    print_farewell();
    Ok(())
}

fn print_farewell() {
    const BLUE: &str = "\x1b[34m";
    const WHITE: &str = "\x1b[97m";
    const DIM: &str = "\x1b[2m";
    const RESET: &str = "\x1b[0m";

    const FAREWELLS: &[&str] = &[
        "May your branches always merge cleanly.",
        "Eight arms, zero attachments.",
        "The wise abbot inks only when necessary.",
        "Go forth and refactor in peace.",
        "Patience is bitter, but its fruit has eight arms.",
        "In the monastery of code, every bug is a koan.",
        "The octopus who grasps at nothing holds everything.",
        "Even an octopus can only solve eight problems at once.",
        "May your deployments be as smooth as tentacles in water.",
        "One need at a time. Unless you have eight arms.",
        "The abbot bows. The tentacles wave.",
        "Ink well, deploy well.",
    ];

    let index = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as usize % FAREWELLS.len())
        .unwrap_or(0);

    println!();
    println!("  {BLUE}▗▄███▄▖{RESET}");
    println!("  {BLUE} █{WHITE}◉ ◉{BLUE}█{RESET}");
    println!("  {BLUE} ⠿ ⠿ ⠿{RESET}");
    println!();
    println!("  {DIM}{}{RESET}", FAREWELLS[index]);
    println!();
}

// =============================================================================
// ENTRY POINT
// =============================================================================

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();
    run_app(cli.addr, cli.scope).await
}
