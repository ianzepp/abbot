//! Monitor view — real-time frame stream table with filtering and detail overlay.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap},
};

use crate::App;
use crate::app::ViewMode;
use crate::widgets::{
    centered_rect, draw_header, draw_statusline, draw_top_nav, draw_view_picker, op_color, truncate,
};

// =============================================================================
// HELPERS
// =============================================================================

/// Extract the room from a frame's trace or data, preferring trace.
fn frame_room(frame: &crate::app::Frame) -> Option<&str> {
    frame
        .trace
        .as_ref()
        .and_then(|t| t.get("room"))
        .and_then(|s| s.as_str())
        .or_else(|| {
            frame
                .data
                .as_ref()
                .and_then(|d| d.get("room"))
                .and_then(|s| s.as_str())
        })
}

/// Map a frame name to a single-char kind badge for the table column.
///
/// N = need, T = task, W = work (tool invocation), R = reply.
fn frame_kind(name: Option<&str>) -> &'static str {
    match name {
        Some(n) if n.starts_with("need:") => "N",
        Some(n) if n.starts_with("task:") => "T",
        Some(n) if n.starts_with("tool:") || n == "chat:tool" => "W",
        Some(n) if n.starts_with("reply:") => "R",
        _ => "",
    }
}

// =============================================================================
// DRAWING
// =============================================================================

pub fn draw_monitor(f: &mut Frame, app: &App) {
    // WHY: 1-char horizontal margins keep table content from touching terminal edges
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
        ViewMode::Frames => " Frames",
        ViewMode::Needs => " Needs",
        ViewMode::Tasks => " Tasks",
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

    let scroll_offset = app.selected.saturating_sub(visible_count.saturating_sub(1));
    let frames: Vec<_> = frames
        .into_iter()
        .skip(scroll_offset)
        .take(visible_count)
        .collect();

    let rows: Vec<Row> = frames
        .iter()
        .enumerate()
        .map(|(i, rec)| {
            let is_selected = scroll_offset + i == app.selected;
            let marker = if is_selected { "●" } else { " " };
            let marker_span = Span::styled(marker, Style::default().fg(theme.selection));

            let time = rec.timestamp.format("%H:%M:%S").to_string();
            let time_span = Span::raw(time);

            let op_span = Span::styled(&rec.frame.op, Style::default().fg(op_color(&rec.frame.op)));

            let kind_text = match &rec.resolved {
                Some(res) => res.clone(),
                None => {
                    let k = frame_kind(rec.frame.name.as_deref());
                    if k.is_empty() {
                        "".to_string()
                    } else {
                        k.to_string()
                    }
                }
            };
            let kind_color = if rec.resolved.is_some() {
                theme.border_green
            } else if !kind_text.is_empty() {
                theme.border_yellow
            } else {
                theme.text_dim
            };
            let kind_span = Span::styled(kind_text, Style::default().fg(kind_color));

            let name = truncate(rec.frame.name.as_deref().unwrap_or(""), 20);
            let name_span = Span::raw(name);

            let room = frame_room(&rec.frame).unwrap_or("?");
            let room_span =
                Span::styled(format!("#{}", room), Style::default().fg(theme.border_cyan));

            // Content: truncated JSON data
            let content = if let Some(data) = &rec.frame.data {
                truncate(&data.to_string(), 60)
            } else {
                String::new()
            };
            let content_span = Span::raw(content);

            let actor = truncate(rec.frame.actor.as_deref().unwrap_or(""), 16);
            let actor_span = Span::styled(actor, Style::default().fg(theme.text_dim));

            let row_style = if is_selected {
                Style::default().bg(theme.panel_header_bg)
            } else {
                Style::default()
            };

            Row::new(vec![
                marker_span,
                time_span,
                op_span,
                kind_span,
                name_span,
                room_span,
                content_span,
                actor_span,
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(1),  // marker
        Constraint::Length(8),  // time
        Constraint::Length(7),  // op
        Constraint::Length(4),  // kind
        Constraint::Length(20), // name
        Constraint::Length(6),  // room
        Constraint::Min(20),    // content (fills)
        Constraint::Length(16), // actor
    ];

    let table = Table::new(rows, widths)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default().fg(theme.text_primary));

    f.render_widget(table, inner);
}

fn draw_detail(f: &mut Frame, app: &App) {
    let area = centered_rect(80, 70, f.area());
    f.render_widget(Clear, area);

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

    if app.selected >= frames.len() {
        return;
    }

    let selected_frame = &frames[app.selected].frame;
    let theme = &app.theme;

    let mut content = String::new();

    // ID
    content.push_str(&format!("ID: {}\n", selected_frame.id));

    // Parent ID
    if let Some(parent_id) = selected_frame.parent_id {
        content.push_str(&format!("Parent: {}\n", parent_id));
    }

    // Timestamp
    content.push_str(&format!("Timestamp: {}\n", selected_frame.ts));

    // Trace
    if let Some(trace) = &selected_frame.trace {
        content.push_str("\nTrace:\n");
        content.push_str(&serde_json::to_string_pretty(trace).unwrap_or_default());
        content.push('\n');
    }

    // Data
    if let Some(data) = &selected_frame.data {
        content.push_str("\nData:\n");
        content.push_str(&serde_json::to_string_pretty(data).unwrap_or_default());
    }

    let title = format!(
        " {} | {} | {} ",
        selected_frame.op,
        selected_frame.name.as_deref().unwrap_or("?"),
        selected_frame.actor.as_deref().unwrap_or("?")
    );

    let para = Paragraph::new(content)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border_cyan)),
        )
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(theme.text_primary));

    f.render_widget(para, area);
}

fn draw_monitor_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let mode_text = match app.view_mode {
        ViewMode::Frames => "all",
        ViewMode::Needs => "needs",
        ViewMode::Tasks => "tasks",
    };

    let counts = format!(
        "[n:{}] [t:{}] [w:{}]",
        app.need_count, app.task_count, app.tool_count
    );

    let left_line = Line::from(format!(" {} — {}", mode_text, counts));
    let right_line = "[a] [n] [t] [p] [j] [k] [Enter] [^C]";

    draw_statusline(f, theme, area, left_line, right_line, theme.border_red);
}
