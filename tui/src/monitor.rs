use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::widgets::{
    centered_rect, draw_header, draw_statusline, draw_top_nav, draw_view_picker, op_color, truncate,
};
use crate::App;

fn frame_scope(frame: &crate::Frame) -> Option<&str> {
    frame
        .trace
        .as_ref()
        .and_then(|t| t.get("scope"))
        .and_then(|s| s.as_str())
        .or_else(|| {
            frame
                .data
                .as_ref()
                .and_then(|d| d.get("scope"))
                .and_then(|s| s.as_str())
        })
}

fn frame_kind(name: Option<&str>) -> &'static str {
    match name {
        Some(n) if n.starts_with("need:") => "N",
        Some(n) if n.starts_with("task:") => "T",
        Some(n) if n.starts_with("tool:") || n == "chat:tool" => "W",
        Some(n) if n.starts_with("reply:") => "R",
        _ => "",
    }
}

pub fn draw_monitor(f: &mut Frame, app: &App) {
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
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(h_chunks[1]);

    draw_top_nav(
        f,
        &app.theme,
        chunks[1],
        app.view,
        app.paused,
        app.queued_count,
        app.tick_count,
        app.connected,
    );
    draw_frames(f, app, chunks[3]);
    draw_monitor_status(f, app, chunks[4]);

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }

    if app.show_detail {
        draw_detail(f, app);
    }
}

fn draw_frames(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let title = match app.view_mode {
        crate::ViewMode::Frames => " Frames",
        crate::ViewMode::Needs => " Needs",
        crate::ViewMode::Tasks => " Tasks",
    };

    let header_area = Rect::new(area.x, area.y, area.width, 3);
    draw_header(f, theme, header_area, title, theme.border_red);

    let inner = Rect::new(
        area.x,
        area.y + 4,
        area.width,
        area.height.saturating_sub(4),
    );

    if app.frames.is_empty() {
        let placeholder =
            Paragraph::new("  Waiting for frames...").style(Style::default().fg(theme.text_dim));
        f.render_widget(placeholder, inner);
        return;
    }

    let visible_count = inner.height as usize;
    let frames: Vec<_> = app
        .frames
        .iter()
        .rev()
        .filter(|rec| match app.view_mode {
            crate::ViewMode::Frames => true,
            crate::ViewMode::Needs => rec
                .frame
                .name
                .as_ref()
                .map(|n| n.starts_with("need:"))
                .unwrap_or(false),
            crate::ViewMode::Tasks => rec
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
            let kind = frame_kind(rec.frame.name.as_deref());
            let resolved = rec
                .resolved
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(kind);

            let scope = frame_scope(&rec.frame)
                .map(|s| {
                    if let Some(hash) = s.strip_prefix("session/") {
                        format!("@{}", &hash[..4.min(hash.len())])
                    } else {
                        format!("#{}", s)
                    }
                })
                .unwrap_or_default();

            let content = rec
                .frame
                .data
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
                Span::styled(format!("{:6}", op), Style::default().fg(op_color(op))),
                Span::styled(
                    format!("{:4}", resolved),
                    if rec.resolved.is_some() {
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::DIM)
                    } else if !kind.is_empty() {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::DIM)
                    } else {
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM)
                    },
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

fn draw_monitor_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let mode_text = match app.view_mode {
        crate::ViewMode::Frames => "all",
        crate::ViewMode::Needs => "needs",
        crate::ViewMode::Tasks => "tasks",
    };

    let left = Line::from(vec![
        Span::styled(
            format!("[{}]", mode_text),
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
        Span::styled(
            format!(" [n:{}]", app.need_count),
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
        Span::styled(
            format!(" [t:{}]", app.task_count),
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
        Span::styled(
            format!(" [w:{}]", app.tool_count),
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
    ]);

    draw_statusline(f, theme, area, left, "[^T] [^C]", theme.border_red);
}

fn draw_detail(f: &mut Frame, app: &App) {
    let theme = &app.theme;
    let frames: Vec<_> = app
        .frames
        .iter()
        .rev()
        .filter(|rec| match app.view_mode {
            crate::ViewMode::Frames => true,
            crate::ViewMode::Needs => rec
                .frame
                .name
                .as_ref()
                .map(|n| n.starts_with("need:"))
                .unwrap_or(false),
            crate::ViewMode::Tasks => rec
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

    let trace_json = rec
        .frame
        .trace
        .as_ref()
        .map(|t| serde_json::to_string_pretty(t).unwrap_or_else(|_| t.to_string()))
        .unwrap_or_else(|| "(no trace)".to_string());

    let data_json = rec
        .frame
        .data
        .as_ref()
        .map(|d| serde_json::to_string_pretty(d).unwrap_or_else(|_| d.to_string()))
        .unwrap_or_else(|| "(no data)".to_string());

    let mut lines = vec![
        format!("id:        {}", rec.frame.id),
        format!(
            "parent_id: {}",
            rec.frame
                .parent_id
                .map(|u| u.to_string())
                .unwrap_or("-".into())
        ),
        format!("time:      {}", rec.timestamp.format("%H:%M:%S%.3f")),
        String::new(),
        "trace:".to_string(),
    ];

    for line in trace_json.lines() {
        lines.push(format!("  {}", line));
    }

    lines.push(String::new());
    lines.push("data:".to_string());

    for line in data_json.lines() {
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
