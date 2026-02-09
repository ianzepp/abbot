//! App entry point — terminal setup, main event loop, and view dispatch.

mod config;
mod logs;
mod monitor;
mod theme;
mod widgets;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Frame as RatatuiFrame, Terminal, backend::CrosstermBackend};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::sync::mpsc;

use config::{ConfigDialog, ConfigEditorState, ConfigFocus, FieldType};
use logs::{LogEntry, LogsFocus, LogsState};
use theme::Theme;

// =============================================================================
// TYPES & STATE
// =============================================================================

#[derive(Parser)]
#[command(name = "abbot-monitor")]
#[command(about = "TUI frame monitor for Abbot")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:8080")]
    addr: String,

    /// Optional unix domain socket for raw frame stream (default: <workspace>/frames.sock)
    #[arg(long)]
    frames_sock: Option<PathBuf>,
}

fn default_frames_sock_from_config() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".abbot").join("frames.sock"))
}

/// Background task: connects to the daemon's UDS frame socket, reconnecting
/// on disconnect with a 1-2s backoff.
async fn run_uds_client(sock: PathBuf, tx: mpsc::Sender<WsEvent>) {
    loop {
        #[cfg(unix)]
        {
            match tokio::net::UnixStream::connect(&sock).await {
                Ok(stream) => {
                    let _ = tx.send(WsEvent::Connected).await;
                    let mut lines = BufReader::new(stream).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(line)
                            && let WsMessage::Frame(frame) = ws_msg
                        {
                            let _ = tx.send(WsEvent::Frame(frame)).await;
                        }
                    }
                    let _ = tx.send(WsEvent::Disconnected).await;
                }
                Err(_) => {
                    let _ = tx.send(WsEvent::Disconnected).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = sock;
            let _ = tx;
            return;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Wire-format frame received from the daemon's UDS stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub id: uuid::Uuid,
    #[serde(default)]
    pub ts: i64,
    pub op: String,
    pub name: Option<String>,
    pub parent_id: Option<uuid::Uuid>,
    pub actor: Option<String>,
    #[serde(default)]
    pub trace: Option<serde_json::Value>,
    pub data: Option<serde_json::Value>,
}

/// Envelope format for messages on the UDS frame stream.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
enum WsMessage {
    #[serde(rename = "connected")]
    Connected {
        #[allow(dead_code)]
        version: String,
    },
    #[serde(rename = "frame")]
    Frame(Frame),
    #[serde(rename = "pong")]
    Pong {
        #[allow(dead_code)]
        timestamp_ms: i64,
    },
    #[serde(rename = "error")]
    Error {
        #[allow(dead_code)]
        message: String,
    },
}

/// A frame with local arrival timestamp and resolution state.
#[derive(Debug, Clone)]
pub struct FrameRecord {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub frame: Frame,
    pub resolved: Option<String>,
}

/// Monitor sub-filter: which frame subset to display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Frames,
    Needs,
    Tasks,
}

/// Top-level view tabs, switchable via number keys or Ctrl-T picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Monitor,
    Config,
    Logs,
}

/// One 500ms bucket of frame op counts for the monitor sparkline.
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

/// Root application state shared across all views.
pub struct App {
    pub frames: VecDeque<FrameRecord>,
    pending: HashMap<uuid::Uuid, usize>,
    timeline: VecDeque<TimelineBucket>,
    pub syscall_ticker: VecDeque<String>,
    pub view_mode: ViewMode,
    pub paused: bool,
    pub selected: usize,
    pub need_count: usize,
    pub task_count: usize,
    pub tool_count: usize,
    pub reply_count: usize,
    pub tick_count: usize,
    pub show_detail: bool,
    pub show_view_picker: bool,
    pub view_picker_selected: usize,
    pub view: View,
    pub connected: bool,
    pub queued_count: usize,
    pub config_editor: ConfigEditorState,
    pub logs: Vec<LogEntry>,
    pub logs_state: LogsState,
    pub dark_mode: bool,
    pub theme: Theme,
}

// =============================================================================
// CHANNEL EVENTS
// =============================================================================

/// Events from the UDS frame stream background task.
enum WsEvent {
    Connected,
    Disconnected,
    Frame(Frame),
}

/// Events from async config load/save and model list fetch tasks.
enum ConfigEvent {
    Loaded(serde_json::Value),
    Saved,
    Error(String),
    ModelOptionsLoaded(Vec<ModelOption>),
    ModelOptionsError(String),
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ProviderModelItem {
    provider: String,
    #[allow(dead_code)]
    fetched_at: String,
    id: String,
    name: Option<String>,
    context_window: Option<u64>,
    input_cost: Option<f64>,
    output_cost: Option<f64>,
}

#[derive(Debug, serde::Deserialize)]
struct ProviderModelsResponse {
    items: Vec<ProviderModelItem>,
}

#[derive(Debug, Clone)]
struct ModelOption {
    id: String,
    display: String,
}

/// Events from async log entry fetch tasks.
enum LogsEvent {
    Loaded(Vec<LogEntry>),
    Error(String),
}

// =============================================================================
// APP CONSTRUCTION & FRAME INGESTION
// =============================================================================

impl App {
    fn new(dark_mode: bool) -> Self {
        Self {
            frames: VecDeque::with_capacity(1000),
            pending: HashMap::new(),
            timeline: VecDeque::with_capacity(120),
            syscall_ticker: VecDeque::with_capacity(50),
            view_mode: ViewMode::Frames,
            paused: false,
            selected: 0,
            need_count: 0,
            task_count: 0,
            tool_count: 0,
            reply_count: 0,
            tick_count: 0,
            show_detail: false,
            show_view_picker: false,
            view_picker_selected: 0,
            view: View::Monitor,
            connected: false,
            queued_count: 0,
            config_editor: ConfigEditorState::new(),
            logs: Vec::new(),
            logs_state: LogsState::new(),
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
            if let Some(seq) = frame
                .data
                .as_ref()
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
                if let Some(&idx) = self.pending.get(parent_id)
                    && let Some(rec) = self.frames.get_mut(idx)
                {
                    rec.resolved = Some(frame.op.clone());
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
            } else if name.starts_with("tool:") || name == "chat:tool" {
                self.tool_count += 1;
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

        if let Some(rec) = self.frames.back() {
            let label = rec.frame.name.as_deref().unwrap_or(&rec.frame.op);
            self.syscall_ticker.push_back(label.to_string());
            while self.syscall_ticker.len() > 50 {
                self.syscall_ticker.pop_front();
            }
        }
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

    fn monitor_total(&self) -> usize {
        self.frames
            .iter()
            .filter(|rec| match self.view_mode {
                ViewMode::Frames => true,
                ViewMode::Needs => rec
                    .frame
                    .name
                    .as_deref()
                    .is_some_and(|n| n.starts_with("need:")),
                ViewMode::Tasks => rec
                    .frame
                    .name
                    .as_deref()
                    .is_some_and(|n| n.starts_with("task:")),
            })
            .count()
    }
}

// =============================================================================
// DRAWING DISPATCH
// =============================================================================

fn draw(f: &mut RatatuiFrame, app: &App) {
    match app.view {
        View::Monitor => monitor::draw_monitor(f, app),
        View::Config => config::draw_config(f, app),
        View::Logs => logs::draw_logs(f, app),
    }
}

// =============================================================================
// ASYNC TASKS
// =============================================================================

fn admin_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

async fn fetch_config(addr: &str, tx: mpsc::Sender<ConfigEvent>) {
    let client = admin_http_client();
    let url = format!("http://{}/admin/config", addr);

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            let _ = tx
                .send(ConfigEvent::Error(format!("Request failed: {}", e)))
                .await;
            return;
        }
    };

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        let error_msg = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            json.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or(&body)
                .to_string()
        } else {
            body
        };
        let _ = tx
            .send(ConfigEvent::Error(format!(
                "HTTP {}: {}",
                status, error_msg
            )))
            .await;
        return;
    }

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        let _ = tx.send(ConfigEvent::Loaded(json)).await;
    }
}

async fn save_config(addr: &str, config: serde_json::Value, tx: mpsc::Sender<ConfigEvent>) {
    let client = admin_http_client();
    let url = format!("http://{}/admin/config", addr);

    let resp = match client
        .put(&url)
        .header("Content-Type", "application/json")
        .json(&config)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let _ = tx
                .send(ConfigEvent::Error(format!("Save failed: {}", e)))
                .await;
            return;
        }
    };

    let status = resp.status();
    if status.is_success() {
        let _ = tx.send(ConfigEvent::Saved).await;
    } else {
        let body = resp.text().await.unwrap_or_default();
        let error_msg = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            json.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or(&body)
                .to_string()
        } else {
            body
        };
        let _ = tx
            .send(ConfigEvent::Error(format!(
                "HTTP {}: {}",
                status, error_msg
            )))
            .await;
    }
}

async fn fetch_provider_models(addr: &str, provider: Option<&str>, tx: mpsc::Sender<ConfigEvent>) {
    let client = admin_http_client();
    let url = format!("http://{}/admin/providers/models", addr);

    let mut req = client.get(&url).query(&[("limit", "2000")]);
    if let Some(p) = provider
        && !p.trim().is_empty()
    {
        req = req.query(&[("provider", p)]);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let _ = tx
                .send(ConfigEvent::ModelOptionsError(format!(
                    "Request failed: {}",
                    e
                )))
                .await;
            return;
        }
    };

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let error_msg = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            json.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or(&body)
                .to_string()
        } else {
            body
        };
        let _ = tx
            .send(ConfigEvent::ModelOptionsError(format!(
                "HTTP {}: {}",
                status, error_msg
            )))
            .await;
        return;
    }

    let resp = match serde_json::from_str::<ProviderModelsResponse>(&body) {
        Ok(r) => r,
        Err(_) => {
            let _ = tx
                .send(ConfigEvent::ModelOptionsError("Invalid response".into()))
                .await;
            return;
        }
    };

    fn fmt_price(cost: Option<f64>) -> String {
        match cost {
            None => "-".to_string(),
            Some(0.0) => "free".to_string(),
            Some(c) => format!("${:.2}", c * 1_000_000.0),
        }
    }

    let mut opts = Vec::new();
    for m in resp.items {
        let full_id = if m.provider == "openrouter" {
            format!("openrouter/{}", m.id.trim_matches('/'))
        } else {
            let native = m.id.split('/').next_back().unwrap_or(m.id.as_str());
            format!("{}/{}", m.provider, native)
        };

        let ctx = m
            .context_window
            .map(|c| format!("{}k", c / 1000))
            .unwrap_or_else(|| "-".to_string());
        let price = format!("{} / {}", fmt_price(m.input_cost), fmt_price(m.output_cost));
        let name = m.name.unwrap_or_default();
        let name = if name.is_empty() {
            "".to_string()
        } else {
            format!(" ({})", name)
        };
        let display = format!("{:<56} {:>13}  ctx:{}{}", full_id, price, ctx, name);
        opts.push(ModelOption {
            id: full_id,
            display,
        });
    }

    if opts.is_empty() {
        let _ = tx
            .send(ConfigEvent::ModelOptionsError(
                "No cached models found (run: abbot providers refresh)".into(),
            ))
            .await;
    } else {
        let _ = tx.send(ConfigEvent::ModelOptionsLoaded(opts)).await;
    }
}

async fn fetch_logs(addr: &str, query_string: &str, tx: mpsc::Sender<LogsEvent>) {
    let client = admin_http_client();
    let url = if query_string.is_empty() {
        format!("http://{}/admin/logs", addr)
    } else {
        format!("http://{}/admin/logs?{}", addr, query_string)
    };

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            let _ = tx
                .send(LogsEvent::Error(format!("Request failed: {}", e)))
                .await;
            return;
        }
    };

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        let error_msg = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            json.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or(&body)
                .to_string()
        } else {
            body
        };
        let _ = tx
            .send(LogsEvent::Error(format!("HTTP {}: {}", status, error_msg)))
            .await;
    } else if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(items) = json.get("items").and_then(|v| v.as_array()) {
            let entries: Vec<LogEntry> = items
                .iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect();
            let _ = tx.send(LogsEvent::Loaded(entries)).await;
        } else {
            let _ = tx.send(LogsEvent::Loaded(Vec::new())).await;
        }
    } else {
        let _ = tx
            .send(LogsEvent::Error("Invalid response".to_string()))
            .await;
    }
}

/// Detects terminal background brightness for theme selection.
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
// MAIN EVENT LOOP
// =============================================================================

async fn run_app(addr: String, frames_sock_cli: Option<PathBuf>) -> io::Result<()> {
    let dark_mode = detect_dark_mode();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(dark_mode);
    let (tx, mut rx) = mpsc::channel::<WsEvent>(100);
    let (config_tx, mut config_rx) = mpsc::channel::<ConfigEvent>(100);
    let (logs_tx, mut logs_rx) = mpsc::channel::<LogsEvent>(100);

    let mut last_view = app.view;

    // Frame stream: UDS only (local trusted client).
    let frames_sock_env = std::env::var("ABBOT_FRAMES_SOCK").ok().map(PathBuf::from);
    let frames_sock = frames_sock_cli
        .or_else(|| frames_sock_env.clone())
        .or_else(default_frames_sock_from_config);

    let Some(sock) = frames_sock else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "frames socket not configured; pass --frames-sock /path/to/frames.sock or ensure ~/.abbot/ exists",
        ));
    };

    tokio::spawn(run_uds_client(sock, tx.clone()));

    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();
    let mut paused_queue: VecDeque<Frame> = VecDeque::new();

    loop {
        terminal.draw(|f| draw(f, &app))?;

        // -- Keyboard handling ------------------------------------------------
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                break;
            }

            if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
                app.show_view_picker = !app.show_view_picker;
                app.view_picker_selected = match app.view {
                    View::Monitor => 0,
                    View::Config => 1,
                    View::Logs => 2,
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
                        app.view_picker_selected = (app.view_picker_selected + 1).min(2);
                    }
                    KeyCode::Char('1') => {
                        app.view = View::Monitor;
                        app.show_view_picker = false;
                    }
                    KeyCode::Char('2') => {
                        app.view = View::Config;
                        app.show_view_picker = false;
                    }
                    KeyCode::Char('3') => {
                        app.view = View::Logs;
                        app.show_view_picker = false;
                    }
                    KeyCode::Enter => {
                        app.view = match app.view_picker_selected {
                            0 => View::Monitor,
                            1 => View::Config,
                            _ => View::Logs,
                        };
                        app.show_view_picker = false;
                    }
                    _ => {}
                }
                continue;
            }

            if app.view == View::Config {
                if app.config_editor.save_confirm {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            app.config_editor.save_confirm = false;
                            let config = app.config_editor.to_json();
                            let addr_clone = addr.clone();
                            let tx_clone = config_tx.clone();
                            tokio::spawn(async move {
                                save_config(&addr_clone, config, tx_clone).await;
                            });
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            app.config_editor.save_confirm = false;
                        }
                        _ => {}
                    }
                } else if app.config_editor.dialog.is_some() {
                    let dialog = app.config_editor.dialog.as_mut().unwrap();
                    match key.code {
                        KeyCode::Esc => {
                            app.config_editor.dialog = None;
                            app.config_editor.focus = ConfigFocus::Fields;
                        }
                        KeyCode::Enter => {
                            let value = dialog.to_value();
                            if let Some(field) = app.config_editor.current_field_mut() {
                                field.value = value;
                            }
                            app.config_editor.dialog = None;
                            app.config_editor.focus = ConfigFocus::Fields;
                        }
                        KeyCode::Up => {
                            if matches!(
                                dialog.field_type,
                                FieldType::Toggle | FieldType::Select | FieldType::Model
                            ) {
                                dialog.selected = dialog.selected.saturating_sub(1);
                                if dialog.field_type == FieldType::Model {
                                    let max =
                                        dialog.model_filtered_indices().len().saturating_sub(1);
                                    dialog.selected = dialog.selected.min(max);
                                }
                            }
                        }
                        KeyCode::Down => {
                            if matches!(dialog.field_type, FieldType::Toggle | FieldType::Select) {
                                dialog.selected = (dialog.selected + 1)
                                    .min(dialog.options.len().saturating_sub(1));
                            } else if dialog.field_type == FieldType::Model {
                                dialog.selected = dialog.selected.saturating_add(1);
                                let max = dialog.model_filtered_indices().len().saturating_sub(1);
                                dialog.selected = dialog.selected.min(max);
                            }
                        }
                        KeyCode::Char('k')
                            if matches!(
                                dialog.field_type,
                                FieldType::Toggle | FieldType::Select
                            ) =>
                        {
                            dialog.selected = dialog.selected.saturating_sub(1);
                        }
                        KeyCode::Char('j')
                            if matches!(
                                dialog.field_type,
                                FieldType::Toggle | FieldType::Select
                            ) =>
                        {
                            dialog.selected =
                                (dialog.selected + 1).min(dialog.options.len().saturating_sub(1));
                        }
                        KeyCode::Char(c) => {
                            if matches!(
                                dialog.field_type,
                                FieldType::Text
                                    | FieldType::Password
                                    | FieldType::Number
                                    | FieldType::Model
                            ) {
                                dialog.input.insert(dialog.cursor, c);
                                dialog.cursor += 1;
                                if dialog.field_type == FieldType::Model {
                                    dialog.selected = 0;
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            if dialog.cursor > 0 {
                                dialog.cursor -= 1;
                                dialog.input.remove(dialog.cursor);
                                if dialog.field_type == FieldType::Model {
                                    dialog.selected = 0;
                                }
                            }
                        }
                        KeyCode::Delete => {
                            if dialog.cursor < dialog.input.len() {
                                dialog.input.remove(dialog.cursor);
                                if dialog.field_type == FieldType::Model {
                                    dialog.selected = 0;
                                }
                            }
                        }
                        KeyCode::Left => {
                            dialog.cursor = dialog.cursor.saturating_sub(1);
                        }
                        KeyCode::Right => {
                            dialog.cursor = (dialog.cursor + 1).min(dialog.input.len());
                        }
                        KeyCode::Home => {
                            dialog.cursor = 0;
                        }
                        KeyCode::End => {
                            dialog.cursor = dialog.input.len();
                        }
                        _ => {}
                    }
                } else if key.code == KeyCode::Char('r')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    app.config_editor.loading = true;
                    app.config_editor.error = None;
                    let addr_clone = addr.clone();
                    let tx_clone = config_tx.clone();
                    tokio::spawn(async move {
                        fetch_config(&addr_clone, tx_clone).await;
                    });
                } else if key.code == KeyCode::Char('s')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    if app.config_editor.is_any_dirty() {
                        match app.config_editor.validate_for_save() {
                            Ok(()) => {
                                app.config_editor.save_confirm = true;
                            }
                            Err(e) => {
                                app.config_editor.error = Some(e);
                            }
                        }
                    }
                } else {
                    match app.config_editor.focus {
                        ConfigFocus::Sections => match key.code {
                            KeyCode::Char('1') => app.view = View::Monitor,
                            KeyCode::Char('2') => {}
                            KeyCode::Char('3') => app.view = View::Logs,
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.config_editor.selected_section =
                                    app.config_editor.selected_section.saturating_sub(1);
                                app.config_editor.selected_field = 0;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                let max = app.config_editor.sections.len().saturating_sub(1);
                                app.config_editor.selected_section =
                                    (app.config_editor.selected_section + 1).min(max);
                                app.config_editor.selected_field = 0;
                            }
                            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                                if !app.config_editor.sections.is_empty() {
                                    app.config_editor.focus = ConfigFocus::Fields;
                                    app.config_editor.selected_field = 0;
                                }
                            }
                            _ => {}
                        },
                        ConfigFocus::Fields => match key.code {
                            KeyCode::Char('1') => app.view = View::Monitor,
                            KeyCode::Char('2') => {}
                            KeyCode::Char('3') => app.view = View::Logs,
                            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                                app.config_editor.focus = ConfigFocus::Sections;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.config_editor.selected_field =
                                    app.config_editor.selected_field.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if let Some(section) = app.config_editor.current_section() {
                                    let max = section.fields.len().saturating_sub(1);
                                    app.config_editor.selected_field =
                                        (app.config_editor.selected_field + 1).min(max);
                                }
                            }
                            KeyCode::Enter => {
                                if let Some((dialog, field_type)) = app
                                    .config_editor
                                    .current_field()
                                    .map(|f| (ConfigDialog::for_field(f), f.field_type))
                                {
                                    app.config_editor.dialog = Some(dialog);
                                    app.config_editor.focus = ConfigFocus::Dialog;

                                    if field_type == FieldType::Model {
                                        let addr_clone = addr.clone();
                                        let tx_clone = config_tx.clone();
                                        tokio::spawn(async move {
                                            fetch_provider_models(&addr_clone, None, tx_clone)
                                                .await;
                                        });
                                    }
                                }
                            }
                            _ => {}
                        },
                        ConfigFocus::Dialog => {}
                    }
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
                    KeyCode::Char('1') => {}
                    KeyCode::Char('2') => app.view = View::Config,
                    KeyCode::Char('3') => app.view = View::Logs,
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
                    KeyCode::Enter => {
                        if app.monitor_total() > 0 {
                            app.show_detail = true;
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.selected = app.selected.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let max = app.monitor_total().saturating_sub(1);
                        app.selected = (app.selected + 1).min(max);
                    }
                    KeyCode::Home => app.selected = 0,
                    _ => {}
                }
            } else if app.view == View::Logs {
                if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    app.logs_state.loading = true;
                    app.logs_state.selected = 0;
                    let addr_clone = addr.clone();
                    let tx_clone = logs_tx.clone();
                    let query = app.logs_state.build_query_string();
                    tokio::spawn(async move {
                        fetch_logs(&addr_clone, &query, tx_clone).await;
                    });
                } else if app.logs_state.show_detail {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                            app.logs_state.show_detail = false;
                        }
                        _ => {}
                    }
                } else {
                    match app.logs_state.focus {
                        LogsFocus::List => match key.code {
                            KeyCode::Char('1') => app.view = View::Monitor,
                            KeyCode::Char('2') => app.view = View::Config,
                            KeyCode::Char('3') => {}
                            KeyCode::Tab => {
                                app.logs_state.focus = LogsFocus::Search;
                            }
                            KeyCode::Enter => {
                                if !app.logs.is_empty() {
                                    app.logs_state.show_detail = true;
                                }
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.logs_state.selected = app.logs_state.selected.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if app.logs_state.selected + 1 < app.logs.len() {
                                    app.logs_state.selected += 1;
                                }
                            }
                            _ => {}
                        },
                        LogsFocus::Search => match key.code {
                            KeyCode::Tab => {
                                app.logs_state.focus = LogsFocus::List;
                            }
                            KeyCode::Esc => {
                                app.logs_state.focus = LogsFocus::List;
                            }
                            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.logs_state.search_field = app.logs_state.search_field.prev();
                            }
                            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.logs_state.search_field = app.logs_state.search_field.next();
                            }
                            KeyCode::Enter => {
                                app.logs_state.loading = true;
                                app.logs_state.selected = 0;
                                let addr_clone = addr.clone();
                                let tx_clone = logs_tx.clone();
                                let query = app.logs_state.build_query_string();
                                tokio::spawn(async move {
                                    fetch_logs(&addr_clone, &query, tx_clone).await;
                                });
                            }
                            KeyCode::Char(c) => {
                                app.logs_state
                                    .current_input_mut()
                                    .handle(tui_input::InputRequest::InsertChar(c));
                            }
                            KeyCode::Backspace => {
                                app.logs_state
                                    .current_input_mut()
                                    .handle(tui_input::InputRequest::DeletePrevChar);
                            }
                            KeyCode::Delete => {
                                app.logs_state
                                    .current_input_mut()
                                    .handle(tui_input::InputRequest::DeleteNextChar);
                            }
                            KeyCode::Left => {
                                app.logs_state
                                    .current_input_mut()
                                    .handle(tui_input::InputRequest::GoToPrevChar);
                            }
                            KeyCode::Right => {
                                app.logs_state
                                    .current_input_mut()
                                    .handle(tui_input::InputRequest::GoToNextChar);
                            }
                            KeyCode::Home => {
                                app.logs_state
                                    .current_input_mut()
                                    .handle(tui_input::InputRequest::GoToStart);
                            }
                            KeyCode::End => {
                                app.logs_state
                                    .current_input_mut()
                                    .handle(tui_input::InputRequest::GoToEnd);
                            }
                            _ => {}
                        },
                    }
                }
            }
        }

        // -- Channel drain & view-switch triggers ----------------------------
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

            // Handle config events
            while let Ok(event) = config_rx.try_recv() {
                match event {
                    ConfigEvent::Loaded(json) => {
                        app.config_editor.load_from_json(json);
                    }
                    ConfigEvent::Saved => {
                        for section in &mut app.config_editor.sections {
                            for field in &mut section.fields {
                                field.original = field.value.clone();
                            }
                        }
                    }
                    ConfigEvent::Error(error) => {
                        app.config_editor.error = Some(error);
                        app.config_editor.loading = false;
                    }
                    ConfigEvent::ModelOptionsLoaded(options) => {
                        if let Some(dialog) = app.config_editor.dialog.as_mut() {
                            dialog.set_model_options(options);
                        }
                    }
                    ConfigEvent::ModelOptionsError(error) => {
                        if let Some(dialog) = app.config_editor.dialog.as_mut() {
                            dialog.set_model_error(error);
                        }
                    }
                }
            }

            if app.view != last_view {
                match app.view {
                    View::Config => {
                        if !app.config_editor.is_any_dirty() && !app.config_editor.loading {
                            app.config_editor.loading = true;
                            app.config_editor.error = None;
                            let addr_clone = addr.clone();
                            let tx_clone = config_tx.clone();
                            tokio::spawn(async move {
                                fetch_config(&addr_clone, tx_clone).await;
                            });
                        }
                    }
                    View::Logs => {
                        if !app.logs_state.loading {
                            app.logs_state.loading = true;
                            app.logs_state.error = None;
                            app.logs_state.selected = 0;
                            let addr_clone = addr.clone();
                            let tx_clone = logs_tx.clone();
                            let query = app.logs_state.build_query_string();
                            tokio::spawn(async move {
                                fetch_logs(&addr_clone, &query, tx_clone).await;
                            });
                        }
                    }
                    _ => {}
                }

                last_view = app.view;
            }

            // Handle logs events
            while let Ok(event) = logs_rx.try_recv() {
                match event {
                    LogsEvent::Loaded(entries) => {
                        app.logs = entries;
                        app.logs_state.loading = false;
                        app.logs_state.error = None;
                    }
                    LogsEvent::Error(error) => {
                        app.logs_state.error = Some(error);
                        app.logs_state.loading = false;
                    }
                }
            }

            if app.view == View::Monitor {
                let max = app.monitor_total().saturating_sub(1);
                if app.selected > max {
                    app.selected = max;
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
    run_app(cli.addr, cli.frames_sock).await
}
