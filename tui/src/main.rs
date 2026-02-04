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
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Row, Table, Wrap},
    Frame as RatatuiFrame, Terminal,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tui_input::Input;

#[derive(Parser)]
#[command(name = "abbot-tui")]
#[command(about = "TUI frame monitor for Abbot")]
struct Cli {
    /// Abbot server address
    #[arg(long, default_value = "127.0.0.1:8080")]
    addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Frame {
    id: uuid::Uuid,
    op: String,
    name: Option<String>,
    parent_id: Option<uuid::Uuid>,
    actor: Option<String>,
    data: Option<serde_json::Value>,
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
struct FrameRecord {
    timestamp: chrono::DateTime<chrono::Local>,
    frame: Frame,
    resolved: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Frames,
    Needs,
    Tasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Monitor,
    Chat,
    Explorer,
    Config,
}

#[derive(Debug, Clone)]
struct ChatMessage {
    role: String,
    content: String,
    timestamp: chrono::DateTime<chrono::Local>,
}

struct App {
    frames: VecDeque<FrameRecord>,
    pending: HashMap<uuid::Uuid, usize>,
    timeline: VecDeque<TimelineBucket>,
    sessions: Vec<Session>,
    view_mode: ViewMode,
    paused: bool,
    selected: usize,
    need_count: usize,
    task_count: usize,
    reply_count: usize,
    tick_count: usize,
    show_detail: bool,
    show_view_picker: bool,
    view_picker_selected: usize,
    view: View,
    compose_input: Input,
    chat_insert_mode: bool,
    chat_messages: Vec<ChatMessage>,
    chat_scroll: usize,
    connected: bool,
    queued_count: usize,
    explorer_selected: usize,
    explorer_tree: Vec<ExplorerNode>,
    config_selected: usize,
    config_wizard: Option<Wizard>,
    config_input: Input,
    dark_mode: bool,
    theme: Theme,
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

struct Session {
    id: String,
    frame_count: usize,
    last_role: String,
    last_content: String,
}

#[derive(Clone)]
struct ExplorerNode {
    name: String,
    is_dir: bool,
    depth: usize,
    expanded: bool,
    content: Option<String>,
}

#[derive(Clone)]
enum WizardStep {
    Text { prompt: String, value: String },
    Password { prompt: String, value: String, masked: bool },
    Select { prompt: String, options: Vec<String>, selected: usize },
    Confirm { prompt: String, value: bool },
    Info { text: String },
}

#[derive(Clone)]
struct Wizard {
    title: String,
    steps: Vec<WizardStep>,
    current: usize,
    completed: bool,
}

#[derive(Clone, Debug)]
enum ConfigCommand {
    AddProvider,
    SetModel,
    ConfigureWorkspace,
    EditTraits,
    ManageMemory,
}

impl ConfigCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::AddProvider => "Add Provider",
            Self::SetModel => "Set Default Model",
            Self::ConfigureWorkspace => "Configure Workspace",
            Self::EditTraits => "Edit Personality Traits",
            Self::ManageMemory => "Manage Memory",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::AddProvider => "Add or update an LLM provider API key",
            Self::SetModel => "Change the default model for conversations",
            Self::ConfigureWorkspace => "Set workspace path and settings",
            Self::EditTraits => "Adjust personality and behavior traits",
            Self::ManageMemory => "View and manage long-term memory",
        }
    }

    fn create_wizard(&self) -> Wizard {
        match self {
            Self::AddProvider => Wizard {
                title: "Add Provider".into(),
                steps: vec![
                    WizardStep::Select {
                        prompt: "Select provider".into(),
                        options: vec![
                            "Anthropic".into(),
                            "OpenAI".into(),
                            "OpenRouter".into(),
                            "Ollama (local)".into(),
                        ],
                        selected: 0,
                    },
                    WizardStep::Password {
                        prompt: "Enter API key".into(),
                        value: String::new(),
                        masked: true,
                    },
                    WizardStep::Confirm {
                        prompt: "Set as default provider?".into(),
                        value: true,
                    },
                ],
                current: 0,
                completed: false,
            },
            Self::SetModel => Wizard {
                title: "Set Default Model".into(),
                steps: vec![
                    WizardStep::Select {
                        prompt: "Select provider".into(),
                        options: vec![
                            "anthropic".into(),
                            "openai".into(),
                            "openrouter".into(),
                            "ollama".into(),
                        ],
                        selected: 0,
                    },
                    WizardStep::Select {
                        prompt: "Select model".into(),
                        options: vec![
                            "claude-sonnet-4-20250514".into(),
                            "claude-opus-4-20250514".into(),
                            "claude-haiku-3-20240307".into(),
                        ],
                        selected: 0,
                    },
                ],
                current: 0,
                completed: false,
            },
            Self::ConfigureWorkspace => Wizard {
                title: "Configure Workspace".into(),
                steps: vec![
                    WizardStep::Text {
                        prompt: "Workspace path".into(),
                        value: "~/projects".into(),
                    },
                    WizardStep::Confirm {
                        prompt: "Enable file watching?".into(),
                        value: true,
                    },
                    WizardStep::Select {
                        prompt: "Default scope".into(),
                        options: vec!["main".into(), "workspace".into(), "session".into()],
                        selected: 0,
                    },
                ],
                current: 0,
                completed: false,
            },
            Self::EditTraits => Wizard {
                title: "Edit Personality Traits".into(),
                steps: vec![
                    WizardStep::Select {
                        prompt: "Verbosity".into(),
                        options: vec!["Concise".into(), "Normal".into(), "Detailed".into()],
                        selected: 1,
                    },
                    WizardStep::Select {
                        prompt: "Formality".into(),
                        options: vec!["Casual".into(), "Professional".into(), "Academic".into()],
                        selected: 1,
                    },
                    WizardStep::Confirm {
                        prompt: "Enable proactive suggestions?".into(),
                        value: true,
                    },
                ],
                current: 0,
                completed: false,
            },
            Self::ManageMemory => Wizard {
                title: "Manage Memory".into(),
                steps: vec![
                    WizardStep::Info {
                        text: "Long-term memories: 42\nShort-term memories: 128\nTotal size: 2.3 MB".into(),
                    },
                    WizardStep::Confirm {
                        prompt: "Clear short-term memory?".into(),
                        value: false,
                    },
                ],
                current: 0,
                completed: false,
            },
        }
    }
}

const CONFIG_COMMANDS: &[ConfigCommand] = &[
    ConfigCommand::AddProvider,
    ConfigCommand::SetModel,
    ConfigCommand::ConfigureWorkspace,
    ConfigCommand::EditTraits,
    ConfigCommand::ManageMemory,
];

struct Theme {
    header_bg: Color,
    panel_header_bg: Color,
    text_primary: Color,
    text_secondary: Color,
    text_dim: Color,
    border_red: Color,
    border_blue: Color,
    border_green: Color,
    border_yellow: Color,
    border_magenta: Color,
    border_cyan: Color,
    selection: Color,
    status_bar_bg: Color,
}

impl Theme {
    fn dark() -> Self {
        Self {
            header_bg: Color::Rgb(40, 40, 40),
            panel_header_bg: Color::Rgb(50, 50, 50),
            text_primary: Color::White,
            text_secondary: Color::Rgb(180, 180, 180),
            text_dim: Color::DarkGray,
            border_red: Color::Red,
            border_blue: Color::Blue,
            border_green: Color::Green,
            border_yellow: Color::Yellow,
            border_magenta: Color::Magenta,
            border_cyan: Color::Cyan,
            selection: Color::Green,
            status_bar_bg: Color::Blue,
        }
    }

    fn light() -> Self {
        Self {
            header_bg: Color::Rgb(220, 220, 220),
            panel_header_bg: Color::Rgb(200, 200, 200),
            text_primary: Color::Rgb(30, 30, 30),
            text_secondary: Color::Rgb(60, 60, 60),
            text_dim: Color::Rgb(120, 120, 120),
            border_red: Color::Rgb(180, 40, 40),
            border_blue: Color::Rgb(40, 80, 180),
            border_green: Color::Rgb(40, 140, 40),
            border_yellow: Color::Rgb(180, 140, 0),
            border_magenta: Color::Rgb(140, 40, 140),
            border_cyan: Color::Rgb(0, 140, 160),
            selection: Color::Rgb(40, 140, 40),
            status_bar_bg: Color::Rgb(60, 100, 180),
        }
    }

    fn for_mode(dark_mode: bool) -> Self {
        if dark_mode { Self::dark() } else { Self::light() }
    }
}

enum WsEvent {
    Connected,
    Disconnected,
    Frame(Frame),
}

impl App {
    fn new(dark_mode: bool) -> Self {
        Self {
            frames: VecDeque::with_capacity(1000),
            pending: HashMap::new(),
            timeline: VecDeque::with_capacity(120),
            sessions: Vec::new(),
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
            view: View::Monitor,
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

        // Capture chat messages for #main
        if let Some(data) = &frame.data {
            let scope = data.get("scope").and_then(|s| s.as_str());
            let kind = data.get("kind").and_then(|k| k.as_str());

            if scope == Some("main") {
                if let Some(kind) = kind {
                    let role = if kind.contains("user") {
                        "user"
                    } else if kind.contains("assistant") {
                        "assistant"
                    } else {
                        ""
                    };

                    if !role.is_empty() {
                        let content = data.get("data")
                            .and_then(|d| d.get("content"))
                            .and_then(|c| c.as_str())
                            .unwrap_or("");

                        if !content.is_empty() {
                            self.chat_messages.push(ChatMessage {
                                role: role.to_string(),
                                content: content.to_string(),
                                timestamp: chrono::Local::now(),
                            });
                        }
                    }
                }
            }
        }

        // Skip terminal response frames - they're shown via parent correlation
        if matches!(frame.op.as_str(), "ok" | "done" | "error") {
            // Still update parent correlation before skipping
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

fn op_color(op: &str) -> Color {
    match op {
        "req" => Color::Blue,
        "ok" | "done" => Color::Green,
        "error" => Color::Red,
        "item" | "progress" => Color::Yellow,
        "cancel" => Color::DarkGray,
        "redirect" => Color::Magenta,
        _ => Color::White,
    }
}

fn draw(f: &mut RatatuiFrame, app: &App) {
    match app.view {
        View::Monitor => draw_monitor(f, app),
        View::Chat => draw_chat(f, app),
        View::Explorer => draw_explorer(f, app),
        View::Config => draw_config(f, app),
    }
}

/// Draw a 3-row header with colored borders on left/right
fn draw_header<'a>(f: &mut RatatuiFrame, app: &App, area: Rect, title: impl Into<Line<'a>>, border_color: Color) {
    let theme = &app.theme;

    // Fill background
    let bg_widget = Paragraph::new("").style(Style::default().bg(theme.header_bg));
    f.render_widget(bg_widget, area);

    // Left border
    let left_border = Paragraph::new("▎\n▎\n▎")
        .style(Style::default().fg(border_color).bg(theme.header_bg));
    f.render_widget(left_border, Rect::new(area.x, area.y, 1, 3));

    // Right border
    let right_border = Paragraph::new("▕\n▕\n▕")
        .style(Style::default().fg(border_color).bg(theme.header_bg));
    f.render_widget(right_border, Rect::new(area.x + area.width - 1, area.y, 1, 3));

    // Title (centered vertically in row 1)
    let title_area = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1);
    let title_line: Line = title.into();
    let title_widget = Paragraph::new(title_line);
    f.render_widget(title_widget, title_area);
}

/// Draw a 1-row status line with colored borders on left/right, gray bg in middle
fn draw_statusline(f: &mut RatatuiFrame, app: &App, area: Rect, left_content: Line, right_content: &str, border_color: Color) {
    let theme = &app.theme;
    let bg = theme.header_bg;

    // Fill gray background
    let bg_widget = Paragraph::new("").style(Style::default().bg(bg));
    f.render_widget(bg_widget, area);

    // Left border
    let left_border = Paragraph::new("▎").style(Style::default().fg(border_color).bg(bg));
    f.render_widget(left_border, Rect::new(area.x, area.y, 1, 1));

    // Right border
    let right_border = Paragraph::new("▕").style(Style::default().fg(border_color).bg(bg));
    f.render_widget(right_border, Rect::new(area.x + area.width - 1, area.y, 1, 1));

    // Left content
    let left_area = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), 1);
    f.render_widget(Paragraph::new(left_content), left_area);

    // Right content
    let right_area = Rect::new(
        area.x + area.width.saturating_sub(right_content.len() as u16 + 2),
        area.y,
        right_content.len() as u16,
        1,
    );
    f.render_widget(Paragraph::new(right_content).style(Style::default().bg(bg).fg(theme.text_primary)), right_area);
}

fn draw_config(f: &mut RatatuiFrame, app: &App) {
    // Horizontal margins
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // top margin
            Constraint::Length(1),  // top nav
            Constraint::Length(1),  // margin
            Constraint::Length(3),  // header
            Constraint::Length(1),  // margin
            Constraint::Min(10),    // panels
            Constraint::Length(1),  // status bar
        ])
        .split(h_chunks[1]);

    draw_top_nav(f, app, chunks[1]);
    draw_config_header(f, app, chunks[3]);

    // Two panels: 1/3 commands, 2/3 wizard
    let panel_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(2, 3),
        ])
        .split(chunks[5]);

    draw_config_commands(f, app, panel_chunks[0]);
    draw_config_wizard(f, app, panel_chunks[1]);

    draw_config_status(f, app, chunks[6]);

    if app.show_view_picker {
        draw_view_picker(f, app);
    }
}

fn draw_config_header(f: &mut RatatuiFrame, app: &App, area: Rect) {
    draw_header(f, app, area, " Configuration", app.theme.border_magenta);
}

fn draw_config_commands(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;

    // Panel header
    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let header = Paragraph::new(" Commands")
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    // Panel content
    let content_area = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

    let in_wizard = app.config_wizard.is_some();

    let rows: Vec<Row> = CONFIG_COMMANDS
        .iter()
        .enumerate()
        .map(|(i, cmd)| {
            let is_selected = i == app.config_selected && !in_wizard;
            let marker = if is_selected { "●" } else { " " };
            let style = if is_selected {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.text_dim)
            };

            Row::new(vec![
                Span::styled(marker, Style::default().fg(theme.border_magenta)),
                Span::styled(format!(" {}", cmd.name()), style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(10),
        ],
    );
    f.render_widget(table, content_area);
}

fn draw_config_wizard(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;

    let Some(wizard) = &app.config_wizard else {
        // No active wizard - show command description
        let header_area = Rect::new(area.x, area.y, area.width, 1);
        let cmd = &CONFIG_COMMANDS[app.config_selected];
        let header = Paragraph::new(format!(" {}", cmd.name()))
            .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
        f.render_widget(header, header_area);

        let content_area = Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), area.height.saturating_sub(3));
        let desc = Paragraph::new(cmd.description())
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(desc, content_area);

        let hint_area = Rect::new(area.x + 1, area.y + 4, area.width.saturating_sub(2), 1);
        let hint = Paragraph::new("Press Enter to start")
            .style(Style::default().fg(theme.border_cyan));
        f.render_widget(hint, hint_area);
        return;
    };

    // Header with wizard title and progress
    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let progress = format!(" {} ({}/{})", wizard.title, wizard.current + 1, wizard.steps.len());
    let header = Paragraph::new(progress)
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    // Render current step
    let content_area = Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), area.height.saturating_sub(3));

    if let Some(step) = wizard.steps.get(wizard.current) {
        match step {
            WizardStep::Text { prompt, value } => {
                let prompt_line = Line::from(vec![
                    Span::styled(prompt, Style::default().fg(theme.text_primary)),
                ]);
                f.render_widget(Paragraph::new(prompt_line), content_area);

                let input_area = Rect::new(content_area.x, content_area.y + 2, content_area.width, 1);
                let input_val = if app.config_input.value().is_empty() { value } else { app.config_input.value() };
                let input_line = format!("> {}", input_val);
                f.render_widget(Paragraph::new(input_line).style(Style::default().fg(theme.text_secondary)), input_area);

                let cursor_x = input_area.x + 2 + app.config_input.visual_cursor() as u16;
                f.set_cursor_position((cursor_x, input_area.y));
            }
            WizardStep::Password { prompt, value, masked } => {
                let prompt_line = Line::from(vec![
                    Span::styled(prompt, Style::default().fg(theme.text_primary)),
                ]);
                f.render_widget(Paragraph::new(prompt_line), content_area);

                let input_area = Rect::new(content_area.x, content_area.y + 2, content_area.width, 1);
                let display_val = if *masked {
                    "*".repeat(app.config_input.value().len().max(value.len()))
                } else {
                    app.config_input.value().to_string()
                };
                let input_line = format!("> {}", display_val);
                f.render_widget(Paragraph::new(input_line).style(Style::default().fg(theme.text_secondary)), input_area);

                let cursor_x = input_area.x + 2 + app.config_input.visual_cursor() as u16;
                f.set_cursor_position((cursor_x, input_area.y));
            }
            WizardStep::Select { prompt, options, selected } => {
                let prompt_line = Line::from(vec![
                    Span::styled(prompt, Style::default().fg(theme.text_primary)),
                ]);
                f.render_widget(Paragraph::new(prompt_line), content_area);

                for (i, opt) in options.iter().enumerate() {
                    let opt_area = Rect::new(content_area.x, content_area.y + 2 + i as u16, content_area.width, 1);
                    let marker = if i == *selected { "●" } else { "○" };
                    let style = if i == *selected {
                        Style::default().fg(theme.border_cyan)
                    } else {
                        Style::default().fg(theme.text_dim)
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("  {} ", marker), style),
                        Span::styled(opt, style),
                    ]);
                    f.render_widget(Paragraph::new(line), opt_area);
                }
            }
            WizardStep::Confirm { prompt, value } => {
                let prompt_line = Line::from(vec![
                    Span::styled(prompt, Style::default().fg(theme.text_primary)),
                ]);
                f.render_widget(Paragraph::new(prompt_line), content_area);

                let opt_area = Rect::new(content_area.x, content_area.y + 2, content_area.width, 1);
                let (yes_style, no_style) = if *value {
                    (Style::default().fg(theme.border_cyan), Style::default().fg(theme.text_dim))
                } else {
                    (Style::default().fg(theme.text_dim), Style::default().fg(theme.border_cyan))
                };
                let line = Line::from(vec![
                    Span::styled(if *value { "  ● " } else { "  ○ " }, yes_style),
                    Span::styled("Yes", yes_style),
                    Span::raw("    "),
                    Span::styled(if !*value { "● " } else { "○ " }, no_style),
                    Span::styled("No", no_style),
                ]);
                f.render_widget(Paragraph::new(line), opt_area);
            }
            WizardStep::Info { text } => {
                let info = Paragraph::new(text.as_str())
                    .style(Style::default().fg(theme.text_primary))
                    .wrap(Wrap { trim: false });
                f.render_widget(info, content_area);
            }
        }
    }
}

fn draw_config_status(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;
    let time = chrono::Local::now().format("%H:%M");

    let cmd_name = CONFIG_COMMANDS.get(app.config_selected)
        .map(|c| c.name())
        .unwrap_or("-");

    let step_info = if let Some(ref wizard) = app.config_wizard {
        format!(" [{}/{}]", wizard.current + 1, wizard.steps.len())
    } else {
        String::new()
    };

    let left = Line::from(vec![
        Span::styled(format!("[{}]", time), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [{}]", cmd_name), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(step_info, Style::default().bg(theme.header_bg).fg(theme.text_primary)),
    ]);

    draw_statusline(f, app, area, left, "[^T] [^C]", theme.border_magenta);
}

fn draw_explorer(f: &mut RatatuiFrame, app: &App) {
    // Horizontal margins
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // top margin
            Constraint::Length(1),  // top nav
            Constraint::Length(1),  // margin
            Constraint::Length(3),  // header
            Constraint::Length(1),  // margin
            Constraint::Min(10),    // panels
            Constraint::Length(1),  // status bar
        ])
        .split(h_chunks[1]);

    draw_top_nav(f, app, chunks[1]);
    draw_explorer_header(f, app, chunks[3]);

    // Two panels: 1/3 tree, 2/3 preview
    let panel_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(2, 3),
        ])
        .split(chunks[5]);

    draw_file_tree(f, app, panel_chunks[0]);
    draw_file_preview(f, app, panel_chunks[1]);

    draw_explorer_status(f, app, chunks[6]);

    if app.show_view_picker {
        draw_view_picker(f, app);
    }
}

fn draw_explorer_header(f: &mut RatatuiFrame, app: &App, area: Rect) {
    draw_header(f, app, area, " Workspace Explorer", app.theme.border_yellow);
}

fn draw_file_tree(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;

    // Panel header
    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let header = Paragraph::new(" Files")
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    // Panel content
    let content_area = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

    // Build visible tree (only show items under expanded parents)
    let visible: Vec<(usize, &ExplorerNode)> = build_visible_tree(&app.explorer_tree);

    let rows: Vec<Row> = visible
        .iter()
        .enumerate()
        .map(|(i, (_, node))| {
            let is_selected = i == app.explorer_selected;
            let indent = "  ".repeat(node.depth);

            let icon = if node.is_dir {
                if node.expanded { "▼ " } else { "▶ " }
            } else {
                "  "
            };

            let marker = if is_selected { "●" } else { " " };
            let style = if is_selected {
                Style::default().fg(theme.text_primary)
            } else if node.is_dir {
                Style::default().fg(theme.border_cyan)
            } else {
                Style::default().fg(theme.text_dim)
            };

            Row::new(vec![
                Span::styled(marker, Style::default().fg(theme.border_yellow)),
                Span::styled(format!("{}{}{}", indent, icon, node.name), style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(10),
        ],
    );
    f.render_widget(table, content_area);
}

fn draw_file_preview(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;

    // Panel header
    let header_area = Rect::new(area.x, area.y, area.width, 1);

    let visible = build_visible_tree(&app.explorer_tree);
    let selected_node = visible.get(app.explorer_selected).map(|(_, n)| *n);

    let title = selected_node
        .map(|n| format!(" {}", n.name))
        .unwrap_or_else(|| " Preview".into());

    let header = Paragraph::new(title)
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    // Panel content
    let content_area = Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), area.height.saturating_sub(3));

    let content = selected_node
        .and_then(|n| n.content.as_ref())
        .map(|c| c.as_str())
        .unwrap_or_else(|| {
            if selected_node.map(|n| n.is_dir).unwrap_or(false) {
                "(directory)"
            } else {
                "(no preview available)"
            }
        });

    let preview = Paragraph::new(content)
        .style(Style::default().fg(theme.text_dim))
        .wrap(Wrap { trim: false });
    f.render_widget(preview, content_area);
}

fn build_visible_tree(tree: &[ExplorerNode]) -> Vec<(usize, &ExplorerNode)> {
    let mut visible = Vec::new();
    let mut skip_until_depth: Option<usize> = None;

    for (i, node) in tree.iter().enumerate() {
        if let Some(skip_depth) = skip_until_depth {
            if node.depth > skip_depth {
                continue;
            } else {
                skip_until_depth = None;
            }
        }

        visible.push((i, node));

        if node.is_dir && !node.expanded {
            skip_until_depth = Some(node.depth);
        }
    }

    visible
}

fn draw_explorer_status(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;
    let time = chrono::Local::now().format("%H:%M");

    // Get selected file name
    let visible = build_visible_tree(&app.explorer_tree);
    let selected_name = visible
        .get(app.explorer_selected)
        .map(|(_, n)| n.name.as_str())
        .unwrap_or("-");

    let left = Line::from(vec![
        Span::styled(format!("[{}]", time), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [{}]", selected_name), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
    ]);

    draw_statusline(f, app, area, left, "[^T] [^C]", theme.border_yellow);
}

fn draw_view_picker(f: &mut RatatuiFrame, app: &App) {
    let theme = &app.theme;
    let area = centered_rect(30, 30, f.area());
    f.render_widget(Clear, area);

    let views = ["Monitor", "Chat", "Explorer", "Config"];

    let items: Vec<ListItem> = views
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let marker = if i == app.view_picker_selected { "● " } else { "  " };
            let style = if i == app.view_picker_selected {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.text_dim)
            };
            ListItem::new(format!(" {} {}", marker, name)).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Switch View ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border_cyan))
                .padding(ratatui::widgets::Padding::uniform(1)),
        );

    f.render_widget(list, area);
}

fn draw_top_nav(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;
    let current_view = app.view;

    let items = [
        ("1", "Monitor", View::Monitor),
        ("2", "Chat", View::Chat),
        ("3", "Explorer", View::Explorer),
        ("4", "Config", View::Config),
    ];

    let spans: Vec<Span> = items
        .iter()
        .flat_map(|(key, name, view)| {
            let is_current = *view == current_view;
            let key_style = Style::default().fg(theme.text_dim);
            let name_style = if is_current {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.text_dim)
            };
            vec![
                Span::styled(format!("[{}] ", key), key_style),
                Span::styled(format!("{}  ", name), name_style),
            ]
        })
        .collect();

    let line = Line::from(spans);
    f.render_widget(Paragraph::new(line), area);

    // Right side: paused, queued, ticks, connected, theme indicator
    let (status_text, status_color) = if app.connected {
        ("●", theme.border_green)
    } else {
        ("●", theme.border_red)
    };

    let mut right_spans: Vec<Span> = Vec::new();

    if app.paused {
        right_spans.push(Span::styled("[PAUSED] ", Style::default().fg(theme.border_red)));
        if app.queued_count > 0 {
            right_spans.push(Span::styled(format!("[q:{}] ", app.queued_count), Style::default().fg(theme.border_yellow)));
        }
    }

    right_spans.push(Span::styled(format!("[t:{}] ", app.tick_count), Style::default().fg(theme.text_dim)));
    right_spans.push(Span::styled(status_text, Style::default().fg(status_color)));

    let theme_icon = if app.dark_mode { " ☾" } else { " ☀" };
    right_spans.push(Span::styled(theme_icon, Style::default().fg(theme.text_dim)));

    let right_line = Line::from(right_spans);
    let right_width = right_line.width() as u16;
    let right_area = Rect::new(
        area.x + area.width.saturating_sub(right_width),
        area.y,
        right_width,
        1,
    );
    f.render_widget(Paragraph::new(right_line), right_area);
}

fn draw_monitor(f: &mut RatatuiFrame, app: &App) {
    // Horizontal margins
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),  // left margin
            Constraint::Min(10),    // content
            Constraint::Length(1),  // right margin
        ])
        .split(f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // top margin
            Constraint::Length(1),  // top nav
            Constraint::Length(1),  // margin
            Constraint::Min(10),    // frames
            Constraint::Length(1),  // status bar
        ])
        .split(h_chunks[1]);

    draw_top_nav(f, app, chunks[1]);
    draw_frames(f, app, chunks[3]);
    draw_monitor_status(f, app, chunks[4]);

    if app.show_view_picker {
        draw_view_picker(f, app);
    }

    if app.show_detail {
        draw_detail(f, app);
    }
}

fn draw_chat_header(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;
    draw_header(f, app, area, " Chat #main", theme.border_blue);

    // Right side: message count, connection status, time
    let (status_text, status_color) = if app.connected {
        ("● connected", theme.border_green)
    } else {
        ("● disconnected", theme.border_red)
    };
    let msg_text = format!("messages: {}  ", app.chat_messages.len());
    let time_text = format!("  {}", chrono::Local::now().format("%H:%M"));
    let right_content = Line::from(vec![
        Span::styled(&msg_text, Style::default().fg(theme.text_primary).bg(theme.header_bg)),
        Span::styled(status_text, Style::default().fg(status_color).bg(theme.header_bg)),
        Span::styled(&time_text, Style::default().fg(theme.text_primary).bg(theme.header_bg)),
        Span::styled(" ", Style::default().bg(theme.header_bg)),
    ]);
    let right_width = msg_text.len() + status_text.len() + time_text.len() + 1;
    let right_area = Rect::new(
        area.x + area.width.saturating_sub(right_width as u16 + 1),
        area.y + 1,
        right_width as u16,
        1,
    );
    f.render_widget(Paragraph::new(right_content), right_area);
}

fn draw_chat(f: &mut RatatuiFrame, app: &App) {
    // Horizontal margins
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),  // left margin
            Constraint::Min(10),    // content
            Constraint::Length(1),  // right margin
        ])
        .split(f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // top margin
            Constraint::Length(1),  // top nav
            Constraint::Length(1),  // margin
            Constraint::Length(3),  // chat header
            Constraint::Length(1),  // margin
            Constraint::Min(5),     // messages
            Constraint::Length(1),  // input
            Constraint::Length(1),  // bottom nav
        ])
        .split(h_chunks[1]);

    draw_top_nav(f, app, chunks[1]);
    draw_chat_header(f, app, chunks[3]);

    // Messages area
    let theme = &app.theme;
    let messages_area = chunks[5];
    let visible_lines = messages_area.height as usize;

    let mut lines: Vec<Line> = Vec::new();
    for msg in &app.chat_messages {
        let time = msg.timestamp.format("%H:%M");
        let (nick, nick_style) = if msg.role == "user" {
            ("you", Style::default().fg(theme.border_cyan))
        } else {
            ("abbot", Style::default().fg(theme.border_green))
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{} ", time), Style::default().fg(theme.text_dim)),
            Span::styled("<", Style::default().fg(theme.text_dim)),
            Span::styled(nick, nick_style),
            Span::styled("> ", Style::default().fg(theme.text_dim)),
            Span::styled(&msg.content, Style::default().fg(theme.text_primary)),
        ]));
    }

    let scroll = if lines.len() > visible_lines {
        lines.len() - visible_lines
    } else {
        0
    };

    let messages = Paragraph::new(lines).scroll((scroll as u16, 0));
    f.render_widget(messages, messages_area);

    // Input line with mode indicator
    let input_area = chunks[6];
    let (mode_indicator, mode_style) = if app.chat_insert_mode {
        ("INSERT ", Style::default().fg(theme.border_green))
    } else {
        ("", Style::default())
    };
    let prompt = if app.chat_insert_mode { "> " } else { "  [i] insert " };
    let input_line = Line::from(vec![
        Span::styled(mode_indicator, mode_style),
        Span::styled(prompt, Style::default().fg(theme.text_dim)),
        Span::styled(app.compose_input.value(), Style::default().fg(theme.text_primary)),
    ]);
    f.render_widget(Paragraph::new(input_line), input_area);

    // Cursor only in insert mode
    if app.chat_insert_mode {
        let cursor_x = input_area.x + mode_indicator.len() as u16 + prompt.len() as u16 + app.compose_input.visual_cursor() as u16;
        f.set_cursor_position((cursor_x, input_area.y));
    }

    // Status bar
    draw_chat_status(f, app, chunks[7]);

    if app.show_view_picker {
        draw_view_picker(f, app);
    }
}

fn draw_timeline(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let title_area = Rect::new(area.x, area.y, area.width, 1);
    let title = Paragraph::new(" Timeline")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_widget(title, title_area);

    let tick_text = format!("ticks:{} ", app.tick_count);
    let tick_width = tick_text.len() as u16;
    let tick_area = Rect::new(
        area.x + area.width.saturating_sub(tick_width),
        area.y,
        tick_width,
        1,
    );
    let tick_label = Paragraph::new(tick_text)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_widget(tick_label, tick_area);

    let inner = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

    if inner.width < 2 || inner.height < 2 {
        return;
    }

    let max_buckets = inner.width as usize;
    let max_height = inner.height.saturating_sub(1) as u16;

    let buckets: Vec<_> = app.timeline.iter().rev().take(max_buckets).collect();
    let max_count = buckets
        .iter()
        .map(|b| {
            b.counts.req + b.counts.ok + b.counts.done + b.counts.error + b.counts.item + b.counts.other
        })
        .max()
        .unwrap_or(1)
        .max(1);

    for (i, bucket) in buckets.iter().enumerate() {
        let x = inner.right().saturating_sub(1 + i as u16);
        if x < inner.left() {
            break;
        }

        let total = bucket.counts.req
            + bucket.counts.ok
            + bucket.counts.done
            + bucket.counts.error
            + bucket.counts.item
            + bucket.counts.other;

        let bar_height = ((total as u32 * max_height as u32) / max_count as u32) as u16;
        let bar_height = bar_height.max(if total > 0 { 1 } else { 0 });

        let mut y = inner.bottom().saturating_sub(1);
        let counts = [
            (bucket.counts.req, Color::Blue),
            (bucket.counts.ok + bucket.counts.done, Color::Green),
            (bucket.counts.item, Color::Yellow),
            (bucket.counts.error, Color::Red),
            (bucket.counts.other, Color::DarkGray),
        ];

        let mut drawn = 0u16;
        for (count, color) in counts {
            if count == 0 {
                continue;
            }
            let segment_height =
                ((count as u32 * bar_height as u32) / total.max(1) as u32).max(1) as u16;
            for _ in 0..segment_height {
                if drawn >= bar_height || y < inner.top() {
                    break;
                }
                f.render_widget(
                    Paragraph::new("▄").style(Style::default().fg(color)),
                    Rect::new(x, y, 1, 1),
                );
                y = y.saturating_sub(1);
                drawn += 1;
            }
        }
    }
}

fn draw_frames(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;
    let title = match app.view_mode {
        ViewMode::Frames => " Frames",
        ViewMode::Needs => " Needs",
        ViewMode::Tasks => " Tasks",
    };

    let header_area = Rect::new(area.x, area.y, area.width, 3);
    draw_header(f, app, header_area, title, theme.border_red);

    // 1 row margin after header, then content
    let inner = Rect::new(area.x, area.y + 4, area.width, area.height.saturating_sub(4));

    if app.frames.is_empty() {
        let placeholder = Paragraph::new("  Waiting for frames...")
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(placeholder, inner);
        return;
    }

    let visible_count = inner.height as usize;
    let frames: Vec<_> = app
        .frames
        .iter()
        .rev()
        .filter(|rec| match app.view_mode {
            ViewMode::Frames => true,
            ViewMode::Needs => rec
                .frame
                .name
                .as_ref()
                .map(|n| n.starts_with("need:"))
                .unwrap_or(false),
            ViewMode::Tasks => rec
                .frame
                .name
                .as_ref()
                .map(|n| n.starts_with("task:"))
                .unwrap_or(false),
        })
        .collect();

    let total_frames = frames.len();
    let scroll_offset = app.selected.saturating_sub(visible_count.saturating_sub(1));
    let frames: Vec<_> = frames
        .into_iter()
        .skip(scroll_offset)
        .take(visible_count)
        .collect();

    let content_width = inner.width.saturating_sub(1 + 8 + 7 + 20 + 6 + 4 + 16) as usize;

    let rows: Vec<Row> = frames
        .iter()
        .enumerate()
        .map(|(i, rec)| {
            let time = rec.timestamp.format("%H:%M:%S").to_string();
            let op = &rec.frame.op;
            let name = rec.frame.name.as_deref().unwrap_or("-");
            let actor = rec.frame.actor.as_deref().unwrap_or("-");
            let resolved = rec.resolved.as_deref().unwrap_or("");

            let scope = rec.frame.data
                .as_ref()
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

            let content = rec.frame.data
                .as_ref()
                .map(|d| {
                    let s = d.to_string();
                    truncate(&s, content_width)
                })
                .unwrap_or_default();

            let is_selected = scroll_offset + i == app.selected;
            let marker = if is_selected { "●" } else { " " };

            Row::new(vec![
                Span::styled(marker, Style::default().fg(Color::Green)),
                Span::raw(time),
                Span::styled(
                    format!("{:6}", op),
                    Style::default().fg(op_color(op)),
                ),
                Span::styled(
                    format!("{:4}", resolved),
                    Style::default().fg(Color::Green).add_modifier(Modifier::DIM),
                ),
                Span::raw(format!("{:20}", name)),
                Span::styled(format!("{:5}", scope), Style::default().fg(Color::Cyan)),
                Span::styled(content, Style::default().fg(Color::DarkGray)),
                Span::raw(actor.to_string()),
            ])
        })
        .collect();

    let _ = total_frames;

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(4),
            Constraint::Length(20),
            Constraint::Length(6),
            Constraint::Min(10),
            Constraint::Length(16),
        ],
    );
    f.render_widget(table, inner);
}

fn draw_sessions(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let title_area = Rect::new(area.x, area.y, area.width, 1);
    let title = Paragraph::new(" Sessions")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_widget(title, title_area);

    let inner = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

    if app.sessions.is_empty() {
        let placeholder = Paragraph::new(" No active sessions")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(placeholder, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .map(|s| {
            let line = Line::from(vec![
                Span::styled(&s.id, Style::default().fg(Color::Cyan)),
                Span::raw(format!(" ({}) ", s.frame_count)),
                Span::styled(&s.last_role, Style::default().fg(Color::Yellow)),
                Span::raw(": "),
                Span::raw(truncate(&s.last_content, 60)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

fn draw_monitor_status(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;
    let time = chrono::Local::now().format("%H:%M");

    let mode_text = match app.view_mode {
        ViewMode::Frames => "all",
        ViewMode::Needs => "needs",
        ViewMode::Tasks => "tasks",
    };

    let left = Line::from(vec![
        Span::styled(format!("[{}]", time), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [{}]", mode_text), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [n:{}]", app.need_count), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [t:{}]", app.task_count), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        if app.paused {
            Span::styled(" [PAUSED]", Style::default().bg(theme.header_bg).fg(theme.text_primary))
        } else {
            Span::styled("", Style::default())
        },
    ]);

    draw_statusline(f, app, area, left, "[^T] [^C]", theme.border_red);
}

fn draw_chat_status(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let theme = &app.theme;
    let time = chrono::Local::now().format("%H:%M");

    let mode_text = if app.chat_insert_mode { "INSERT" } else { "NORMAL" };

    let left = Line::from(vec![
        Span::styled(format!("[{}]", time), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(" [#main]", Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [msgs:{}]", app.chat_messages.len()), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [{}]", mode_text), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
    ]);

    draw_statusline(f, app, area, left, "[^T] [^C]", theme.border_blue);
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn draw_detail(f: &mut RatatuiFrame, app: &App) {
    let theme = &app.theme;
    let frames: Vec<_> = app
        .frames
        .iter()
        .rev()
        .filter(|rec| match app.view_mode {
            ViewMode::Frames => true,
            ViewMode::Needs => rec
                .frame
                .name
                .as_ref()
                .map(|n| n.starts_with("need:"))
                .unwrap_or(false),
            ViewMode::Tasks => rec
                .frame
                .name
                .as_ref()
                .map(|n| n.starts_with("task:"))
                .unwrap_or(false),
        })
        .collect();

    let Some(rec) = frames.get(app.selected) else {
        return;
    };

    let area = centered_rect(80, 70, f.area());
    f.render_widget(Clear, area);

    let title = format!(
        " {} | {} | {} ",
        rec.frame.op,
        rec.frame.name.as_deref().unwrap_or("-"),
        rec.frame.actor.as_deref().unwrap_or("-")
    );

    let content = rec
        .frame
        .data
        .as_ref()
        .map(|d| serde_json::to_string_pretty(d).unwrap_or_else(|_| d.to_string()))
        .unwrap_or_else(|| "(no data)".to_string());

    let mut lines = vec![
        format!("id:        {}", rec.frame.id),
        format!("parent_id: {}", rec.frame.parent_id.map(|u| u.to_string()).unwrap_or("-".into())),
        format!("time:      {}", rec.timestamp.format("%H:%M:%S%.3f")),
        String::new(),
        "data:".to_string(),
    ];
    for line in content.lines() {
        lines.push(format!("  {}", line));
    }

    let paragraph = Paragraph::new(lines.join("\n"))
        .style(Style::default().fg(theme.text_primary))
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border_red))
                .padding(ratatui::widgets::Padding::uniform(1)),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

async fn send_message(addr: &str, scope: &str, content: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    Ok(())
}

fn detect_dark_mode() -> bool {
    use std::io::{Read, Write};
    use std::time::Duration;

    // Send OSC 11 query for background color
    print!("\x1b]11;?\x1b\\");
    if std::io::stdout().flush().is_err() {
        return true; // Default to dark
    }

    // Need to briefly enable raw mode to read response
    if enable_raw_mode().is_err() {
        return true;
    }

    let result = (|| {
        // Poll for response with timeout
        if !event::poll(Duration::from_millis(100)).unwrap_or(false) {
            return None;
        }

        // Read raw bytes - response comes as key events in raw mode
        let mut response = String::new();
        while event::poll(Duration::from_millis(10)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                if let KeyCode::Char(c) = key.code {
                    response.push(c);
                }
            }
        }

        // Parse rgb:RRRR/GGGG/BBBB or rgb:RR/GG/BB
        let rgb_start = response.find("rgb:")?;
        let rgb_part = &response[rgb_start + 4..];
        let parts: Vec<&str> = rgb_part.split('/').collect();
        if parts.len() >= 3 {
            // Take first 2 hex chars of each component
            let r_str = &parts[0][..2.min(parts[0].len())];
            let g_str = &parts[1][..2.min(parts[1].len())];
            let b_part = parts[2];
            let b_end = b_part.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(b_part.len());
            let b_str = &b_part[..2.min(b_end)];

            let r = u8::from_str_radix(r_str, 16).ok()?;
            let g = u8::from_str_radix(g_str, 16).ok()?;
            let b = u8::from_str_radix(b_str, 16).ok()?;

            // Luminance formula
            let luminance = r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114;
            return Some(luminance < 128.0);
        }
        None
    })();

    let _ = disable_raw_mode();

    result.unwrap_or(true) // Default to dark mode
}

async fn run_app(addr: String) -> io::Result<()> {
    // Detect terminal background before entering TUI mode
    let dark_mode = detect_dark_mode();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(dark_mode);
    let (tx, mut rx) = mpsc::channel::<WsEvent>(100);

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
                    // Ctrl+C to quit from anywhere
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        break;
                    }

                    // Ctrl+T to show view picker from anywhere
                    if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        app.show_view_picker = !app.show_view_picker;
                        app.view_picker_selected = match app.view {
                            View::Monitor => 0,
                            View::Chat => 1,
                            View::Explorer => 2,
                            View::Config => 3,
                        };
                        continue;
                    }

                    // Handle view picker if open
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
                                app.view = View::Monitor;
                                app.show_view_picker = false;
                            }
                            KeyCode::Char('2') => {
                                app.view = View::Chat;
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
                                    0 => View::Monitor,
                                    1 => View::Chat,
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
                            // Insert mode: all keys go to input
                            match key.code {
                                KeyCode::Esc => {
                                    app.chat_insert_mode = false;
                                }
                                KeyCode::Enter => {
                                    let msg = app.compose_input.value().to_string();
                                    if !msg.is_empty() {
                                        let addr_clone = addr.clone();
                                        tokio::spawn(async move {
                                            let _ = send_message(&addr_clone, "main", &msg).await;
                                        });
                                    }
                                    app.compose_input.reset();
                                    app.chat_insert_mode = false;
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
                            // Normal mode: navigation keys work
                            match key.code {
                                KeyCode::Char('i') => {
                                    app.chat_insert_mode = true;
                                }
                                KeyCode::Char('1') => app.view = View::Monitor,
                                KeyCode::Char('2') => {} // Already in Chat
                                KeyCode::Char('3') => app.view = View::Explorer,
                                KeyCode::Char('4') => app.view = View::Config,
                                _ => {}
                            }
                        }
                    } else if app.view == View::Config {
                        if let Some(ref mut wizard) = app.config_wizard {
                            // Inside wizard
                            match key.code {
                                KeyCode::Esc => {
                                    app.config_wizard = None;
                                    app.config_input.reset();
                                }
                                KeyCode::Enter => {
                                    // Save current step value and advance
                                    if wizard.current + 1 < wizard.steps.len() {
                                        wizard.current += 1;
                                        app.config_input.reset();
                                    } else {
                                        // Wizard complete
                                        wizard.completed = true;
                                        app.config_wizard = None;
                                        app.config_input.reset();
                                    }
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    if let Some(step) = wizard.steps.get_mut(wizard.current) {
                                        match step {
                                            WizardStep::Select { selected, options, .. } => {
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
                            // Command selection
                            match key.code {
                                // Top nav: view switching
                                KeyCode::Char('1') => app.view = View::Monitor,
                                KeyCode::Char('2') => app.view = View::Chat,
                                KeyCode::Char('3') => app.view = View::Explorer,
                                KeyCode::Char('4') => {} // Already in Config
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
                            // Top nav: view switching
                            KeyCode::Char('1') => app.view = View::Monitor,
                            KeyCode::Char('2') => app.view = View::Chat,
                            KeyCode::Char('3') => {} // Already in Explorer
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
                            // Top nav: view switching
                            KeyCode::Char('1') => {} // Already in Monitor
                            KeyCode::Char('2') => app.view = View::Chat,
                            KeyCode::Char('3') => app.view = View::Explorer,
                            KeyCode::Char('4') => app.view = View::Config,
                            // Bottom nav: monitor filters
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
