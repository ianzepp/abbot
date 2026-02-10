//! Room rendering — chat transcript with scrolling + input line.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, AppView, EntryKind, MessageStatus, Mode};
use crate::markdown;

pub fn draw_room(f: &mut Frame, app: &App, area: Rect) {
    if app.active_view == AppView::Hands {
        draw_hands_view(f, app, area);
        return;
    }
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

    if room.messages.is_empty() && room.streaming_buf.is_empty() && !room.pending {
        let hint = "  No messages yet. Press i to start typing.";
        let p = Paragraph::new(hint).style(Style::default().fg(theme.text_dim));
        f.render_widget(p, area);
        return;
    }

    // Content lives indented 4 chars: " ⎿  " for first line, "    " for continuation
    let content_indent = 4_usize;
    let content_width = (area.width as usize).saturating_sub(content_indent);
    let mut lines: Vec<Line> = Vec::new();
    let mut block_count = 0_usize;

    for entry in &room.messages {
        if entry.kind == EntryKind::Activity {
            if !app.show_activity {
                continue;
            }
            // Activity: continuation line with no header, dim style
            let dim = Style::default().fg(theme.text_dim);
            let mut spans: Vec<Span> = vec![Span::styled(" \u{23BF}  ", dim)];
            let text_lines = markdown::wrap_plain(&entry.content, dim, content_width);
            if let Some(first) = text_lines.into_iter().next() {
                spans.extend(first.spans);
            }
            lines.push(Line::from(spans));
            continue;
        }

        // Blank separator between message blocks (not before first)
        if block_count > 0 {
            lines.push(Line::from(""));
        }
        block_count += 1;

        let time = entry.timestamp.format("%H:%M").to_string();

        // Determine bullet + color based on kind + status
        let (bullet, bullet_color, label, content_style) = match entry.kind {
            EntryKind::User => {
                let (b, c) = match entry.status {
                    MessageStatus::Pending => ("\u{23F1}", theme.border_cyan), // ⏱
                    MessageStatus::Failed => ("\u{2717}", theme.border_red),   // ✗
                    _ => ("\u{23FA}", theme.border_cyan),                      // ⏺
                };
                (b, c, "you", Style::default().fg(theme.text_primary))
            }
            EntryKind::Assistant => (
                "\u{23FA}",
                theme.border_green,
                "abbot",
                Style::default().fg(theme.text_secondary),
            ),
            EntryKind::System => (
                "\u{23FA}",
                theme.border_yellow,
                "sys",
                Style::default().fg(theme.border_yellow),
            ),
            EntryKind::Mind => (
                "\u{23FA}",
                theme.border_magenta,
                "mind",
                Style::default().fg(theme.text_secondary),
            ),
            EntryKind::Activity => unreachable!(),
        };

        // Header line: "{bullet} [{HH:MM}] {label}:" with optional right-aligned seq badge
        let header_style = Style::default().fg(bullet_color);
        let header_text = format!("{} [{}] {}:", bullet, time, label);
        if let Some(seq) = entry.seq {
            let badge = format!("[{}]", seq);
            let left_len = header_text.chars().count();
            let right_len = badge.chars().count();
            let total = area.width as usize;
            if left_len + right_len <= total {
                let spaces = total.saturating_sub(left_len + right_len);
                lines.push(Line::from(vec![
                    Span::styled(header_text, header_style),
                    Span::raw(" ".repeat(spaces)),
                    Span::styled(badge, Style::default().fg(theme.text_dim)),
                ]));
            } else {
                lines.push(Line::from(vec![Span::styled(header_text, header_style)]));
            }
        } else {
            lines.push(Line::from(vec![Span::styled(header_text, header_style)]));
        }

        if entry.kind == EntryKind::Assistant && !entry.activity.is_empty() {
            let dim = Style::default().fg(theme.text_dim);
            for line in &entry.activity {
                let mut spans: Vec<Span> = vec![Span::styled(" \u{23BF}  ", dim)];
                let text_lines = markdown::wrap_plain(line, dim, content_width);
                if let Some(first) = text_lines.into_iter().next() {
                    spans.extend(first.spans);
                }
                lines.push(Line::from(spans));
            }
        }

        // Content lines with continuation prefix
        let content_lines = if matches!(entry.kind, EntryKind::Assistant | EntryKind::Mind) {
            markdown::render_markdown(&entry.content, content_style, theme.code_fg, content_width)
        } else {
            markdown::wrap_plain(&entry.content, content_style, content_width)
        };

        for (i, content_line) in content_lines.into_iter().enumerate() {
            let prefix = if i == 0 { " \u{23BF}  " } else { "    " };
            let mut spans: Vec<Span> = vec![Span::styled(prefix, content_style)];
            spans.extend(content_line.spans);
            lines.push(Line::from(spans));
        }
    }

    // Streaming buffer / thinking state
    if !room.streaming_buf.is_empty() || room.pending {
        let time = chrono::Local::now().format("%H:%M").to_string();
        let green_style = Style::default().fg(theme.border_green);
        let content_style = Style::default().fg(theme.text_secondary);

        if room.streaming_buf.is_empty() {
            // Thinking state: pending with no content yet
            if block_count > 0 {
                lines.push(Line::from(""));
            }
            let status = room.status_text.as_deref().unwrap_or("[thinking..]");
            lines.push(Line::from(vec![Span::styled(
                format!("\u{23F1} [{}] abbot:", time),
                green_style,
            )]));
            lines.push(Line::from(vec![
                Span::styled(" \u{23BF}  ", Style::default().fg(theme.text_dim)),
                Span::styled(status.to_string(), Style::default().fg(theme.text_dim)),
            ]));
            if !room.pending_activity.is_empty() {
                let dim = Style::default().fg(theme.text_dim);
                for line in &room.pending_activity {
                    let mut spans: Vec<Span> = vec![Span::styled(" \u{23BF}  ", dim)];
                    let text_lines = markdown::wrap_plain(line, dim, content_width);
                    if let Some(first) = text_lines.into_iter().next() {
                        spans.extend(first.spans);
                    }
                    lines.push(Line::from(spans));
                }
            }
        } else {
            // Content arriving
            if block_count > 0 {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(vec![Span::styled(
                format!("\u{23FA} [{}] abbot:", time),
                green_style,
            )]));

            if !room.pending_activity.is_empty() {
                let dim = Style::default().fg(theme.text_dim);
                for line in &room.pending_activity {
                    let mut spans: Vec<Span> = vec![Span::styled(" \u{23BF}  ", dim)];
                    let text_lines = markdown::wrap_plain(line, dim, content_width);
                    if let Some(first) = text_lines.into_iter().next() {
                        spans.extend(first.spans);
                    }
                    lines.push(Line::from(spans));
                }
            }

            let content_lines = markdown::render_markdown(
                &room.streaming_buf,
                content_style,
                theme.code_fg,
                content_width,
            );

            for (i, content_line) in content_lines.into_iter().enumerate() {
                let prefix = if i == 0 { " \u{23BF}  " } else { "    " };
                let mut spans: Vec<Span> = vec![Span::styled(prefix, content_style)];
                spans.extend(content_line.spans);
                lines.push(Line::from(spans));
            }

            // Cursor on last content line
            lines.push(Line::from(Span::styled(
                "    \u{258C}",
                Style::default().fg(theme.text_dim),
            )));
        }
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

fn draw_hands_view(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let content_indent = 4_usize;
    let content_width = (area.width as usize).saturating_sub(content_indent);
    let mut lines: Vec<Line> = Vec::new();

    let mut entries: Vec<&crate::app::HandLogEntry> = app.hand_log.iter().collect();
    entries.sort_by(|a, b| {
        let an = parse_hand_num(&a.actor);
        let bn = parse_hand_num(&b.actor);
        bn.cmp(&an).then_with(|| b.actor.cmp(&a.actor))
    });

    if entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No hand activity yet.",
            Style::default().fg(theme.text_dim),
        )));
    } else {
        for (block_count, entry) in entries.into_iter().enumerate() {
            if block_count > 0 {
                lines.push(Line::from(""));
            }

            let time = entry.timestamp.format("%H:%M").to_string();
            let header_style = Style::default().fg(theme.border_magenta);
            lines.push(Line::from(vec![Span::styled(
                format!("\u{23FA} [{}] {}:", time, entry.actor),
                header_style,
            )]));

            let mut parts: Vec<String> = Vec::new();
            if let Some(tool) = &entry.tool {
                parts.push(tool.clone());
            }
            if let Some(summary) = &entry.summary {
                parts.push(summary.clone());
            }
            let content = if parts.is_empty() {
                "(started)".to_string()
            } else {
                parts.join(" • ")
            };

            let content_lines =
                markdown::wrap_plain(&content, Style::default().fg(theme.text_dim), content_width);
            for (i, content_line) in content_lines.into_iter().enumerate() {
                let prefix = if i == 0 { " \u{23BF}  " } else { "    " };
                let mut spans: Vec<Span> =
                    vec![Span::styled(prefix, Style::default().fg(theme.text_dim))];
                spans.extend(content_line.spans);
                lines.push(Line::from(spans));
            }
        }
    }

    let visible = area.height as usize;
    let total = lines.len();
    let start = total.saturating_sub(visible);
    let end = total.min(start + visible);
    let visible_lines: Vec<Line> = lines[start..end].to_vec();

    let p = Paragraph::new(visible_lines);
    f.render_widget(p, area);
}

fn parse_hand_num(actor: &str) -> u64 {
    actor
        .strip_prefix("hand/")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
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

    let prompt = if app.input_pending { "  " } else { "> " };
    let input_val = app.input.value();
    let text_color = if app.input_pending {
        theme.text_dim
    } else {
        theme.text_primary
    };

    // Horizontal scroll: keep cursor visible within the available width
    let prompt_len = prompt.len();
    let visible_width = (input_area.width as usize).saturating_sub(prompt_len);
    let cursor_pos = app.input.visual_cursor();
    let scroll_offset = if cursor_pos >= visible_width {
        cursor_pos - visible_width + 1
    } else {
        0
    };
    let visible_text: String = input_val.chars().skip(scroll_offset).collect();
    let display = format!("{}{}", prompt, visible_text);

    let input_widget = Paragraph::new(display).style(Style::default().fg(text_color));
    f.render_widget(input_widget, input_area);

    // Position cursor (hide when input is pending)
    if !app.input_pending && input_area.width > 0 {
        let cursor_x = input_area.x + prompt_len as u16 + (cursor_pos - scroll_offset) as u16;
        f.set_cursor_position((
            cursor_x.min(input_area.x + input_area.width - 1),
            input_area.y,
        ));
    }
}
