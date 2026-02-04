mod chat;
mod config;
mod explorer;
mod monitor;
mod theme;
mod widgets;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{backend::CrosstermBackend, Frame as RatatuiFrame, Terminal};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tui_input::Input;

use config::{Wizard, WizardStep, CONFIG_COMMANDS};
use explorer::{build_visible_tree, ExplorerNode};
use theme::Theme;

#[derive(Parser)]
#[command(name = "abbot-tui")]
#[command(about = "TUI frame monitor for Abbot")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:8080")]
    addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub id: uuid::Uuid,
    pub op: String,
    pub name: Option<String>,
    pub parent_id: Option<uuid::Uuid>,
    pub actor: Option<String>,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
enum WsMessage {
    #[serde(rename = "connected")]
    Connected { version: String },
    #[serde(rename = "frame")]
    Frame(Frame),
    #[serde(rename = "pong")]
    Pong { timestamp_ms: i64 },
    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Debug, Clone)]
pub struct FrameRecord {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub frame: Frame,
    pub resolved: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Frames,
    Needs,
    Tasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Monitor,
    Chat,
    Explorer,
    Config,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Local>,
}

struct TimelineBucket {
    timestamp: Instant,
    counts: FrameCounts,
}

#[derive(Default, Clone)]
struct FrameCounts {
    req: u16,
    ok: u16,
    done: u16,
    error: u16,
    item: u16,
    other: u16,
}

pub struct App {
    pub frames: VecDeque<FrameRecord>,
    pending: HashMap<uuid::Uuid, usize>,
    timeline: VecDeque<TimelineBucket>,
    pub view_mode: ViewMode,
    pub paused: bool,
    pub selected: usize,
    pub need_count: usize,
    pub task_count: usize,
    pub reply_count: usize,
    pub tick_count: usize,
    pub show_detail: bool,
    pub show_view_picker: bool,
    pub view_picker_selected: usize,
    pub view: View,
    pub compose_input: Input,
    pub chat_insert_mode: bool,
    pub chat_messages: Vec<ChatMessage>,
    pub chat_scroll: usize,
    pub connected: bool,
    pub queued_count: usize,
    pub explorer_selected: usize,
    pub explorer_tree: Vec<ExplorerNode>,
    pub config_selected: usize,
    pub config_wizard: Option<Wizard>,
    pub config_input: Input,
    pub dark_mode: bool,
    pub theme: Theme,
}

enum WsEvent {
    Connected,
    Disconnected,
    Frame(Frame),
}

enum ChatEvent {
    AssistantMessage(String),
    Error(String),
}

impl App {
    fn new(dark_mode: bool) -> Self {
        Self {
            frames: VecDeque::with_capacity(1000),
            pending: HashMap::new(),
            timeline: VecDeque::with_capacity(120),
            view_mode: ViewMode::Frames,
            paused: false,
            selected: 0,
            need_count: 0,
            task_count: 0,
            reply_count: 0,
            tick_count: 0,
            show_detail: false,
            show_view_picker: false,
            view_picker_selected: 0,
            view: View::Chat,
            compose_input: Input::default(),
            chat_insert_mode: false,
            chat_messages: Vec::new(),
            chat_scroll: 0,
            connected: false,
            queued_count: 0,
            explorer_selected: 0,
            explorer_tree: vec![
                ExplorerNode { name: "src/".into(), is_dir: true, depth: 0, expanded: true, content: None },
                ExplorerNode { name: "main.rs".into(), is_dir: false, depth: 1, expanded: false, content: Some("fn main() {\n    println!(\"Hello, world!\");\n}".into()) },
                ExplorerNode { name: "lib.rs".into(), is_dir: false, depth: 1, expanded: false, content: Some("pub mod utils;\npub mod config;".into()) },
                ExplorerNode { name: "utils/".into(), is_dir: true, depth: 1, expanded: false, content: None },
                ExplorerNode { name: "docs/".into(), is_dir: true, depth: 0, expanded: false, content: None },
                ExplorerNode { name: "README.md".into(), is_dir: false, depth: 0, expanded: false, content: Some("# My Project\n\nThis is a sample project.\n\n## Features\n\n- Feature 1\n- Feature 2".into()) },
                ExplorerNode { name: "Cargo.toml".into(), is_dir: false, depth: 0, expanded: false, content: Some("[package]\nname = \"myproject\"\nversion = \"0.1.0\"\nedition = \"2021\"".into()) },
            ],
            config_selected: 0,
            config_wizard: None,
            config_input: Input::default(),
            dark_mode,
            theme: Theme::for_mode(dark_mode),
        }
    }

    fn push_frame(&mut self, frame: Frame) {
        let is_tick = frame.op == "event"
            && frame
                .data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(|k| k.as_str())
                == Some("SIGTICK");

        if is_tick {
            if let Some(seq) = frame.data.as_ref()
                .and_then(|d| d.get("seq"))
                .and_then(|s| s.as_u64())
            {
                self.tick_count = seq as usize;
            }
            self.advance_timeline();
            return;
        }

        if matches!(frame.op.as_str(), "ok" | "done" | "error") {
            if let Some(parent_id) = &frame.parent_id {
                if let Some(&idx) = self.pending.get(parent_id) {
                    if let Some(rec) = self.frames.get_mut(idx) {
                        rec.resolved = Some(frame.op.clone());
                    }
                }
                self.pending.remove(parent_id);
            }
            return;
        }

        let now = chrono::Local::now();

        if let Some(name) = &frame.name {
            if name.starts_with("need:") {
                self.need_count += 1;
            } else if name.starts_with("task:") {
                self.task_count += 1;
            } else if name.starts_with("reply:") {
                self.reply_count += 1;
            }
        }

        let idx = self.frames.len();
        if frame.op == "req" {
            self.pending.insert(frame.id, idx);
        }

        let op = frame.op.clone();
        self.frames.push_back(FrameRecord {
            timestamp: now,
            frame,
            resolved: None,
        });

        if self.frames.len() > 1000 {
            self.frames.pop_front();
            self.pending.retain(|_, v| *v > 0);
            for v in self.pending.values_mut() {
                *v = v.saturating_sub(1);
            }
        }

        self.update_timeline(&op);
    }

    fn advance_timeline(&mut self) {
        let now = Instant::now();
        let bucket_duration = Duration::from_millis(500);

        if self.timeline.is_empty()
            || now.duration_since(self.timeline.back().unwrap().timestamp) >= bucket_duration
        {
            self.timeline.push_back(TimelineBucket {
                timestamp: now,
                counts: FrameCounts::default(),
            });
        }

        while self.timeline.len() > 120 {
            self.timeline.pop_front();
        }
    }

    fn update_timeline(&mut self, op: &str) {
        self.advance_timeline();

        if let Some(bucket) = self.timeline.back_mut() {
            match op {
                "req" => bucket.counts.req += 1,
                "ok" => bucket.counts.ok += 1,
                "done" => bucket.counts.done += 1,
                "error" => bucket.counts.error += 1,
                "item" | "progress" => bucket.counts.item += 1,
                _ => bucket.counts.other += 1,
            }
        }
    }
}

fn draw(f: &mut RatatuiFrame, app: &App) {
    match app.view {
        View::Monitor => monitor::draw_monitor(f, app),
        View::Chat => chat::draw_chat(f, app),
        View::Explorer => explorer::draw_explorer(f, app),
        View::Config => config::draw_config(f, app),
    }
}

async fn send_message(
    addr: &str,
    scope: &str,
    content: &str,
    chat_tx: mpsc::Sender<ChatEvent>,
) {
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/chat/completions", addr);

    let body = serde_json::json!({
        "model": "abbot",
        "messages": [
            {"role": "user", "content": content}
        ],
        "stream": false,
        "scope": scope
    });

    let resp = match client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let _ = chat_tx.send(ChatEvent::Error(format!("Request failed: {}", e))).await;
            return;
        }
    };

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        // Try to extract error message from JSON response
        let error_msg = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            json.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or(&body)
                .to_string()
        } else {
            body
        };
        let _ = chat_tx.send(ChatEvent::Error(format!("HTTP {}: {}", status, error_msg))).await;
        return;
    }

    // Parse response to extract assistant message
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(content) = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
        {
            let _ = chat_tx.send(ChatEvent::AssistantMessage(content.to_string())).await;
        }
    }
}

fn detect_dark_mode() -> bool {
    use std::io::Write;

    print!("\x1b]11;?\x1b\\");
    if std::io::stdout().flush().is_err() {
        return true;
    }

    if enable_raw_mode().is_err() {
        return true;
    }

    let result = (|| {
        if !event::poll(Duration::from_millis(100)).unwrap_or(false) {
            return None;
        }

        let mut response = String::new();
        while event::poll(Duration::from_millis(10)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                if let KeyCode::Char(c) = key.code {
                    response.push(c);
                }
            }
        }

        let rgb_start = response.find("rgb:")?;
        let rgb_part = &response[rgb_start + 4..];
        let parts: Vec<&str> = rgb_part.split('/').collect();
        if parts.len() >= 3 {
            let r_str = &parts[0][..2.min(parts[0].len())];
            let g_str = &parts[1][..2.min(parts[1].len())];
            let b_part = parts[2];
            let b_end = b_part.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(b_part.len());
            let b_str = &b_part[..2.min(b_end)];

            let r = u8::from_str_radix(r_str, 16).ok()?;
            let g = u8::from_str_radix(g_str, 16).ok()?;
            let b = u8::from_str_radix(b_str, 16).ok()?;

            let luminance = r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114;
            return Some(luminance < 128.0);
        }
        None
    })();

    let _ = disable_raw_mode();

    result.unwrap_or(true)
}

async fn run_app(addr: String) -> io::Result<()> {
    let dark_mode = detect_dark_mode();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(dark_mode);
    let (tx, mut rx) = mpsc::channel::<WsEvent>(100);
    let (chat_tx, mut chat_rx) = mpsc::channel::<ChatEvent>(100);

    let ws_url = format!("ws://{}/ws", addr);
    tokio::spawn(async move {
        loop {
            match connect_async(&ws_url).await {
                Ok((ws_stream, _)) => {
                    let _ = tx.send(WsEvent::Connected).await;
                    let (_, mut read) = ws_stream.split();
                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                                    if let WsMessage::Frame(frame) = ws_msg {
                                        let _ = tx.send(WsEvent::Frame(frame)).await;
                                    }
                                }
                            }
                            Ok(Message::Close(_)) => break,
                            Err(_) => break,
                            _ => {}
                        }
                    }
                    let _ = tx.send(WsEvent::Disconnected).await;
                }
                Err(_) => {
                    let _ = tx.send(WsEvent::Disconnected).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();
    let mut paused_queue: VecDeque<Frame> = VecDeque::new();

    loop {
        terminal.draw(|f| draw(f, &app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        break;
                    }

                    if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        app.show_view_picker = !app.show_view_picker;
                        app.view_picker_selected = match app.view {
                            View::Chat => 0,
                            View::Monitor => 1,
                            View::Explorer => 2,
                            View::Config => 3,
                        };
                        continue;
                    }

                    if app.show_view_picker {
                        match key.code {
                            KeyCode::Esc => app.show_view_picker = false,
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.view_picker_selected = app.view_picker_selected.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                app.view_picker_selected = (app.view_picker_selected + 1).min(3);
                            }
                            KeyCode::Char('1') => {
                                app.view = View::Chat;
                                app.show_view_picker = false;
                            }
                            KeyCode::Char('2') => {
                                app.view = View::Monitor;
                                app.show_view_picker = false;
                            }
                            KeyCode::Char('3') => {
                                app.view = View::Explorer;
                                app.show_view_picker = false;
                            }
                            KeyCode::Char('4') => {
                                app.view = View::Config;
                                app.show_view_picker = false;
                            }
                            KeyCode::Enter => {
                                app.view = match app.view_picker_selected {
                                    0 => View::Chat,
                                    1 => View::Monitor,
                                    2 => View::Explorer,
                                    _ => View::Config,
                                };
                                app.show_view_picker = false;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.view == View::Chat {
                        if app.chat_insert_mode {
                            match key.code {
                                KeyCode::Esc => {
                                    app.chat_insert_mode = false;
                                }
                                KeyCode::Enter => {
                                    let msg = app.compose_input.value().to_string();
                                    if !msg.is_empty() {
                                        // Add user message to chat immediately
                                        app.chat_messages.push(ChatMessage {
                                            role: "user".to_string(),
                                            content: msg.clone(),
                                            timestamp: chrono::Local::now(),
                                        });
                                        // Send to server
                                        let addr_clone = addr.clone();
                                        let chat_tx_clone = chat_tx.clone();
                                        tokio::spawn(async move {
                                            send_message(&addr_clone, "main", &msg, chat_tx_clone).await;
                                        });
                                    }
                                    app.compose_input.reset();
                                }
                                KeyCode::Char(c) => {
                                    app.compose_input.handle(tui_input::InputRequest::InsertChar(c));
                                }
                                KeyCode::Backspace => {
                                    app.compose_input.handle(tui_input::InputRequest::DeletePrevChar);
                                }
                                KeyCode::Delete => {
                                    app.compose_input.handle(tui_input::InputRequest::DeleteNextChar);
                                }
                                KeyCode::Left => {
                                    app.compose_input.handle(tui_input::InputRequest::GoToPrevChar);
                                }
                                KeyCode::Right => {
                                    app.compose_input.handle(tui_input::InputRequest::GoToNextChar);
                                }
                                KeyCode::Home => {
                                    app.compose_input.handle(tui_input::InputRequest::GoToStart);
                                }
                                KeyCode::End => {
                                    app.compose_input.handle(tui_input::InputRequest::GoToEnd);
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('i') => {
                                    app.chat_insert_mode = true;
                                }
                                KeyCode::Char('1') => {}
                                KeyCode::Char('2') => app.view = View::Monitor,
                                KeyCode::Char('3') => app.view = View::Explorer,
                                KeyCode::Char('4') => app.view = View::Config,
                                _ => {}
                            }
                        }
                    } else if app.view == View::Config {
                        if let Some(ref mut wizard) = app.config_wizard {
                            match key.code {
                                KeyCode::Esc => {
                                    app.config_wizard = None;
                                    app.config_input.reset();
                                }
                                KeyCode::Enter => {
                                    if wizard.current + 1 < wizard.steps.len() {
                                        wizard.current += 1;
                                        app.config_input.reset();
                                    } else {
                                        wizard.completed = true;
                                        app.config_wizard = None;
                                        app.config_input.reset();
                                    }
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    if let Some(step) = wizard.steps.get_mut(wizard.current) {
                                        match step {
                                            WizardStep::Select { selected, .. } => {
                                                *selected = selected.saturating_sub(1);
                                            }
                                            WizardStep::Confirm { value, .. } => {
                                                *value = true;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if let Some(step) = wizard.steps.get_mut(wizard.current) {
                                        match step {
                                            WizardStep::Select { selected, options, .. } => {
                                                *selected = (*selected + 1).min(options.len().saturating_sub(1));
                                            }
                                            WizardStep::Confirm { value, .. } => {
                                                *value = false;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                KeyCode::Left => {
                                    if let Some(WizardStep::Confirm { value, .. }) = wizard.steps.get_mut(wizard.current) {
                                        *value = true;
                                    }
                                }
                                KeyCode::Right => {
                                    if let Some(WizardStep::Confirm { value, .. }) = wizard.steps.get_mut(wizard.current) {
                                        *value = false;
                                    }
                                }
                                KeyCode::Char(c) => {
                                    if let Some(step) = wizard.steps.get(wizard.current) {
                                        match step {
                                            WizardStep::Text { .. } | WizardStep::Password { .. } => {
                                                app.config_input.handle(tui_input::InputRequest::InsertChar(c));
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                KeyCode::Backspace => {
                                    app.config_input.handle(tui_input::InputRequest::DeletePrevChar);
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('1') => app.view = View::Chat,
                                KeyCode::Char('2') => app.view = View::Monitor,
                                KeyCode::Char('3') => app.view = View::Explorer,
                                KeyCode::Char('4') => {}
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.config_selected = app.config_selected.saturating_sub(1);
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    app.config_selected = (app.config_selected + 1).min(CONFIG_COMMANDS.len() - 1);
                                }
                                KeyCode::Enter => {
                                    let cmd = &CONFIG_COMMANDS[app.config_selected];
                                    app.config_wizard = Some(cmd.create_wizard());
                                    app.config_input.reset();
                                }
                                _ => {}
                            }
                        }
                    } else if app.view == View::Explorer {
                        let visible_count = build_visible_tree(&app.explorer_tree).len();
                        match key.code {
                            KeyCode::Char('1') => app.view = View::Chat,
                            KeyCode::Char('2') => app.view = View::Monitor,
                            KeyCode::Char('3') => {}
                            KeyCode::Char('4') => app.view = View::Config,
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.explorer_selected = app.explorer_selected.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if app.explorer_selected + 1 < visible_count {
                                    app.explorer_selected += 1;
                                }
                            }
                            KeyCode::Enter => {
                                let visible = build_visible_tree(&app.explorer_tree);
                                if let Some((tree_idx, node)) = visible.get(app.explorer_selected) {
                                    let tree_idx = *tree_idx;
                                    let is_dir = node.is_dir;
                                    drop(visible);
                                    if is_dir {
                                        app.explorer_tree[tree_idx].expanded = !app.explorer_tree[tree_idx].expanded;
                                    }
                                }
                            }
                            KeyCode::Home => app.explorer_selected = 0,
                            _ => {}
                        }
                    } else if app.show_detail {
                        match key.code {
                            KeyCode::Esc | KeyCode::Enter => {
                                app.show_detail = false;
                            }
                            _ => {}
                        }
                    } else if app.view == View::Monitor {
                        match key.code {
                            KeyCode::Char('1') => app.view = View::Chat,
                            KeyCode::Char('2') => {}
                            KeyCode::Char('3') => app.view = View::Explorer,
                            KeyCode::Char('4') => app.view = View::Config,
                            KeyCode::Char('a') => {
                                app.view_mode = ViewMode::Frames;
                                app.selected = 0;
                            }
                            KeyCode::Char('n') => {
                                app.view_mode = ViewMode::Needs;
                                app.selected = 0;
                            }
                            KeyCode::Char('t') => {
                                app.view_mode = ViewMode::Tasks;
                                app.selected = 0;
                            }
                            KeyCode::Char('p') => app.paused = !app.paused,
                            KeyCode::Enter => app.show_detail = true,
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.selected = app.selected.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                app.selected = app.selected.saturating_add(1);
                            }
                            KeyCode::Home => app.selected = 0,
                            _ => {}
                        }
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            while let Ok(event) = rx.try_recv() {
                match event {
                    WsEvent::Connected => app.connected = true,
                    WsEvent::Disconnected => app.connected = false,
                    WsEvent::Frame(frame) => {
                        if app.paused {
                            paused_queue.push_back(frame);
                            if paused_queue.len() > 10000 {
                                paused_queue.pop_front();
                            }
                            app.queued_count = paused_queue.len();
                        } else {
                            while let Some(queued) = paused_queue.pop_front() {
                                app.push_frame(queued);
                            }
                            app.queued_count = 0;
                            app.push_frame(frame);
                        }
                    }
                }
            }

            // Handle chat events (responses and errors from HTTP requests)
            while let Ok(event) = chat_rx.try_recv() {
                match event {
                    ChatEvent::AssistantMessage(content) => {
                        app.chat_messages.push(ChatMessage {
                            role: "assistant".to_string(),
                            content,
                            timestamp: chrono::Local::now(),
                        });
                    }
                    ChatEvent::Error(error) => {
                        app.chat_messages.push(ChatMessage {
                            role: "error".to_string(),
                            content: error,
                            timestamp: chrono::Local::now(),
                        });
                    }
                }
            }

            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();
    run_app(cli.addr).await
}
