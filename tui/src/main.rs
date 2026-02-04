use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
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

impl App {
    fn new() -> Self {
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
            self.tick_count += 1;
            self.advance_timeline();
            return;
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(f.area());

    draw_timeline(f, app, chunks[0]);
    // chunks[1] is top margin for Frames
    draw_frames(f, app, chunks[2]);
    // chunks[3] is bottom margin for Frames
    draw_sessions(f, app, chunks[4]);
    draw_status(f, app, chunks[5]);

    if app.show_detail {
        draw_detail(f, app);
    }
}

fn draw_timeline(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let title_area = Rect::new(area.x, area.y, area.width, 1);
    let title = Paragraph::new(" Timeline")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_widget(title, title_area);

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
    let title = match app.view_mode {
        ViewMode::Frames => " Frames",
        ViewMode::Needs => " Needs",
        ViewMode::Tasks => " Tasks",
    };
    let title_area = Rect::new(area.x, area.y, area.width, 1);
    let title_widget = Paragraph::new(title)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_widget(title_widget, title_area);

    let inner = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

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

    let content_width = inner.width.saturating_sub(2 + 8 + 7 + 20 + 14 + 4) as usize;

    let rows: Vec<Row> = frames
        .iter()
        .enumerate()
        .map(|(i, rec)| {
            let time = rec.timestamp.format("%H:%M:%S").to_string();
            let op = &rec.frame.op;
            let name = rec.frame.name.as_deref().unwrap_or("-");
            let actor = rec.frame.actor.as_deref().unwrap_or("-");
            let resolved = rec.resolved.as_deref().unwrap_or("");

            let content = rec.frame.data
                .as_ref()
                .map(|d| {
                    let s = d.to_string();
                    truncate(&s, content_width)
                })
                .unwrap_or_default();

            let is_selected = scroll_offset + i == app.selected;
            let marker = if is_selected { "->" } else { "  " };

            Row::new(vec![
                Span::styled(marker, Style::default().fg(Color::Yellow)),
                Span::raw(time),
                Span::styled(
                    format!("{:6}", op),
                    Style::default().fg(op_color(op)),
                ),
                Span::raw(format!("{:20}", name)),
                Span::styled(
                    format!("{:4}", resolved),
                    Style::default().fg(Color::Green).add_modifier(Modifier::DIM),
                ),
                Span::styled(content, Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{:>12}", actor)),
            ])
        })
        .collect();

    let _ = total_frames;

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(20),
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(14),
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

fn draw_status(f: &mut RatatuiFrame, app: &App, area: Rect) {
    let mode_indicator = if app.paused { " PAUSED " } else { "" };

    let status = Line::from(vec![
        Span::styled(
            " [1] ",
            Style::default().bg(Color::DarkGray).fg(Color::White),
        ),
        Span::raw(format!("Needs:{:<4} ", app.need_count)),
        Span::styled(
            " [2] ",
            Style::default().bg(Color::DarkGray).fg(Color::White),
        ),
        Span::raw(format!("Tasks:{:<4} ", app.task_count)),
        Span::styled(
            " [3] ",
            Style::default().bg(Color::DarkGray).fg(Color::White),
        ),
        Span::raw(format!("Replies:{:<4} ", app.reply_count)),
        Span::raw("  "),
        Span::styled(
            " [p] ",
            Style::default().bg(Color::DarkGray).fg(Color::White),
        ),
        Span::raw("pause "),
        Span::styled(
            " [q] ",
            Style::default().bg(Color::DarkGray).fg(Color::White),
        ),
        Span::raw("quit "),
        Span::styled(
            mode_indicator,
            Style::default().bg(Color::Red).fg(Color::White),
        ),
        Span::raw("  "),
        Span::styled(
            format!(" ticks:{} ", app.tick_count),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let paragraph = Paragraph::new(status);
    f.render_widget(paragraph, area);
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
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

async fn run_app(addr: String) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let (tx, mut rx) = mpsc::channel::<Frame>(100);

    let ws_url = format!("ws://{}/ws", addr);
    tokio::spawn(async move {
        loop {
            match connect_async(&ws_url).await {
                Ok((ws_stream, _)) => {
                    let (_, mut read) = ws_stream.split();
                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                                    if let WsMessage::Frame(frame) = ws_msg {
                                        let _ = tx.send(frame).await;
                                    }
                                }
                            }
                            Ok(Message::Close(_)) => break,
                            Err(_) => break,
                            _ => {}
                        }
                    }
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| draw(f, &app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.show_detail {
                        match key.code {
                            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                                app.show_detail = false;
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Char('p') => app.paused = !app.paused,
                            KeyCode::Enter => app.show_detail = true,
                            KeyCode::Char('1') => {
                                app.view_mode = ViewMode::Needs;
                                app.selected = 0;
                            }
                            KeyCode::Char('2') => {
                                app.view_mode = ViewMode::Tasks;
                                app.selected = 0;
                            }
                            KeyCode::Char('3') | KeyCode::Char('0') => {
                                app.view_mode = ViewMode::Frames;
                                app.selected = 0;
                            }
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
            while let Ok(frame) = rx.try_recv() {
                if !app.paused {
                    app.push_frame(frame);
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
