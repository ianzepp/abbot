//! Room rendering — chat transcript with scrolling + input line.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, EntryKind, MessageStatus, Mode};

pub fn draw_room(f: &mut Frame, app: &App, area: Rect) {
    let in_insert = app.mode == Mode::Insert;
    let input_height = if in_insert { 2 } else { 0 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                   // top margin
            Constraint::Min(1),                      // transcript
            Constraint::Length(input_height as u16), // input (only in insert mode)
            Constraint::Length(1),                   // bottom margin
        ])
        .split(area);

    draw_transcript(f, app, chunks[1]);

    if in_insert {
        draw_input(f, app, chunks[2]);
    }
}

fn draw_transcript(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let room = app.current_room();

    if room.messages.is_empty() && room.streaming_buf.is_empty() {
        let hint = if room.pending {
            "  Waiting for response..."
        } else {
            "  No messages yet. Press i to start typing."
        };
        let p = Paragraph::new(hint).style(Style::default().fg(theme.text_dim));
        f.render_widget(p, area);
        return;
    }

    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    for entry in &room.messages {
        if entry.kind == EntryKind::Activity && !app.show_activity {
            continue;
        }

        let time = entry.timestamp.format("%H:%M").to_string();
        let (prefix, prefix_style, content_style) = match entry.kind {
            EntryKind::User => (
                "you",
                Style::default().fg(theme.border_cyan),
                Style::default().fg(theme.text_primary),
            ),
            EntryKind::Assistant => (
                "abbot",
                Style::default().fg(theme.border_green),
                Style::default().fg(theme.text_secondary),
            ),
            EntryKind::Activity => (
                "  ~",
                Style::default().fg(theme.text_dim),
                Style::default().fg(theme.text_dim),
            ),
            EntryKind::System => (
                "sys",
                Style::default().fg(theme.border_yellow),
                Style::default().fg(theme.border_yellow),
            ),
        };

        // Status indicator for user messages
        let status_icon = if entry.kind == EntryKind::User {
            match entry.status {
                MessageStatus::Pending => " ⏱",
                MessageStatus::Sent => " ✓",
                MessageStatus::Failed => " ✗",
                MessageStatus::None => "",
            }
        } else {
            ""
        };

        let header = format!("  {} {:>6}{} > ", time, prefix, status_icon);
        let header_width = header.chars().count();

        for (i, text_line) in entry.content.lines().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled(header.clone(), prefix_style),
                    Span::styled(
                        truncate_line(text_line, width.saturating_sub(header_width)),
                        content_style,
                    ),
                ]));
            } else {
                let indent = " ".repeat(header_width);
                lines.push(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(
                        truncate_line(text_line, width.saturating_sub(header_width)),
                        content_style,
                    ),
                ]));
            }
        }
    }

    // Show streaming buffer if present
    if !room.streaming_buf.is_empty() {
        let time = chrono::Local::now().format("%H:%M").to_string();
        let header = format!("  {} {:>6} > ", time, "abbot");
        let header_width = header.chars().count();

        for (i, text_line) in room.streaming_buf.lines().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled(header.clone(), Style::default().fg(theme.border_green)),
                    Span::styled(
                        truncate_line(text_line, width.saturating_sub(header_width)),
                        Style::default().fg(theme.text_secondary),
                    ),
                ]));
            } else {
                let indent = " ".repeat(header_width);
                lines.push(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(
                        truncate_line(text_line, width.saturating_sub(header_width)),
                        Style::default().fg(theme.text_secondary),
                    ),
                ]));
            }
        }

        // Blinking cursor indicator
        lines.push(Line::from(Span::styled(
            format!("{}▌", " ".repeat(header_width)),
            Style::default().fg(theme.text_dim),
        )));
    }

    // Scroll: show last N lines that fit
    let visible = area.height as usize;
    let total = lines.len();
    let scroll = room.scroll_offset.min(total.saturating_sub(visible));
    let start = total.saturating_sub(visible + scroll);
    let end = (start + visible).min(total);
    let visible_lines: Vec<Line> = lines[start..end].to_vec();

    let p = Paragraph::new(visible_lines);
    f.render_widget(p, area);

    // Scrollback indicator when scrolled up
    if room.scroll_offset > 0 && area.height > 0 {
        let label = "-- more --";
        let label_len = label.len() as u16;
        let x = area.x + area.width.saturating_sub(label_len) / 2;
        let y = area.y + area.height - 1;
        let overlay = Paragraph::new(label).style(Style::default().fg(theme.text_dim));
        f.render_widget(overlay, Rect::new(x, y, label_len, 1));
    }
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    // Separator line
    if area.height >= 2 {
        let sep_area = Rect::new(area.x, area.y, area.width, 1);
        let sep = Paragraph::new("─".repeat(area.width as usize))
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(sep, sep_area);
    }

    let input_area = Rect::new(
        area.x,
        area.y + 1.min(area.height.saturating_sub(1)),
        area.width,
        1,
    );

    let prompt = "> ";
    let input_val = app.input.value();
    let display = format!("{}{}", prompt, input_val);
    let input_widget = Paragraph::new(display).style(Style::default().fg(theme.text_primary));
    f.render_widget(input_widget, input_area);

    // Position cursor
    if input_area.width > 0 {
        let cursor_x = input_area.x + prompt.len() as u16 + app.input.visual_cursor() as u16;
        f.set_cursor_position((
            cursor_x.min(input_area.x + input_area.width - 1),
            input_area.y,
        ));
    }
}

fn truncate_line(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else if max <= 3 {
        s.chars().take(max).collect()
    } else {
        let truncated: String = s.chars().take(max - 3).collect();
        format!("{}...", truncated)
    }
}
