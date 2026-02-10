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
use crate::theme::Theme;

/// Render an activity line with a colored icon prefix if present.
fn render_activity_line<'a>(content: &str, theme: &Theme, content_width: usize) -> Line<'a> {
    let dim = Style::default().fg(theme.text_dim);

    // Detect tool icon prefix and colorize
    let (prefix, text) = if let Some(rest) = content.strip_prefix("\u{2713} ") {
        (" \u{2713}  ", rest)
    } else if let Some(rest) = content.strip_prefix("\u{2717} ") {
        (" \u{2717}  ", rest)
    } else if let Some(rest) = content.strip_prefix("~ ") {
        (" ~  ", rest)
    } else {
        (" \u{23BF}  ", content)
    };

    let icon_style = if content.starts_with('\u{2713}') {
        Style::default().fg(theme.border_green)
    } else if content.starts_with('\u{2717}') {
        Style::default().fg(theme.border_red)
    } else if content.starts_with('~') {
        Style::default().fg(theme.border_yellow)
    } else {
        dim
    };

    let mut spans: Vec<Span> = vec![Span::styled(prefix.to_string(), icon_style)];
    let text_lines = markdown::wrap_plain(text, dim, content_width);
    if let Some(first) = text_lines.into_iter().next() {
        spans.extend(first.spans);
    }
    Line::from(spans)
}

pub fn draw_room(f: &mut Frame, app: &App, area: Rect) {
    match app.active_view {
        AppView::Chat => {}
        AppView::Rooms => {
            draw_rooms_view(f, app, area);
            return;
        }
        AppView::Hands => {
            draw_hands_view(f, app, area);
            return;
        }
        AppView::Needs => {
            draw_ems_view(f, app, area, &app.ems_needs);
            return;
        }
        AppView::Wants => {
            draw_ems_view(f, app, area, &app.ems_wants);
            return;
        }
        AppView::Memories => {
            draw_ems_view(f, app, area, &app.ems_memories);
            return;
        }
    }

    if app.mode == Mode::Insert {
        // Insert mode: transcript + spacer + status bar + input (3 rows)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // transcript
                Constraint::Length(1), // spacer
                Constraint::Length(1), // status bar
                Constraint::Length(3), // input area (space + input + space)
            ])
            .split(area);

        draw_transcript(f, app, chunks[0]);
        crate::ui::draw_status(f, app, chunks[2]);
        draw_input(f, app, chunks[3]);
    } else {
        // Normal mode: transcript + spacer + status bar + frame ticker
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // transcript
                Constraint::Length(1), // spacer
                Constraint::Length(1), // status bar
                Constraint::Length(3), // frame ticker
            ])
            .split(area);

        draw_transcript(f, app, chunks[0]);
        crate::ui::draw_status(f, app, chunks[2]);
        draw_ticker(f, app, chunks[3]);
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
            lines.push(render_activity_line(&entry.content, theme, content_width));
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
            for line in &entry.activity {
                lines.push(render_activity_line(line, theme, content_width));
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
                for line in &room.pending_activity {
                    lines.push(render_activity_line(line, theme, content_width));
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
                for line in &room.pending_activity {
                    lines.push(render_activity_line(line, theme, content_width));
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

fn draw_rooms_view(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let dim = Style::default().fg(theme.text_dim);
    let mut lines: Vec<Line> = Vec::new();

    if app.rooms_list.is_empty() {
        lines.push(Line::from(Span::styled("  No rooms data yet.", dim)));
    } else {
        // Header
        lines.push(Line::from(vec![Span::styled(
            format!("  {:<40} {:>10} {:>10}", "Room", "Last Seq", "Frames"),
            Style::default().fg(theme.text_primary),
        )]));
        lines.push(Line::from(Span::styled(
            format!("  {}", "\u{2500}".repeat(62)),
            dim,
        )));

        for info in &app.rooms_list {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {:<40} {:>10} {:>10}",
                    truncate_str(&info.room, 40),
                    info.last_seq,
                    info.frame_count,
                ),
                dim,
            )));
        }
    }

    let visible = area.height as usize;
    let total = lines.len();
    let start = total.saturating_sub(visible);
    let end = total.min(start + visible);
    let visible_lines: Vec<Line> = lines[start..end].to_vec();
    f.render_widget(Paragraph::new(visible_lines), area);
}

fn draw_ems_view(f: &mut Frame, app: &App, area: Rect, items: &[crate::app::EmsEntity]) {
    let theme = &app.theme;
    let dim = Style::default().fg(theme.text_dim);
    let mut lines: Vec<Line> = Vec::new();

    if items.is_empty() {
        lines.push(Line::from(Span::styled("  No entities yet.", dim)));
    } else {
        // Header
        lines.push(Line::from(vec![Span::styled(
            format!(
                "  {:<10} {:<4} {:<16} {:<16} {}",
                "Status", "Pri", "Room", "Updated", "Prompt"
            ),
            Style::default().fg(theme.text_primary),
        )]));
        lines.push(Line::from(Span::styled(
            format!(
                "  {}",
                "\u{2500}".repeat((area.width as usize).saturating_sub(4).max(60))
            ),
            dim,
        )));

        let prompt_width = (area.width as usize).saturating_sub(52).max(10);
        for entity in items {
            let updated = if entity.updated_at.len() > 16 {
                &entity.updated_at[..16]
            } else {
                &entity.updated_at
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "  {:<10} {:<4} {:<16} {:<16} {}",
                    truncate_str(&entity.status, 10),
                    entity.priority,
                    truncate_str(&entity.room, 16),
                    updated,
                    truncate_str(&entity.prompt, prompt_width),
                ),
                dim,
            )));
        }
    }

    let visible = area.height as usize;
    let total = lines.len();
    let start = total.saturating_sub(visible);
    let end = total.min(start + visible);
    let visible_lines: Vec<Line> = lines[start..end].to_vec();
    f.render_widget(Paragraph::new(visible_lines), area);
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}\u{2026}", truncated)
    }
}

fn parse_hand_num(actor: &str) -> u64 {
    actor
        .strip_prefix("hand/")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    // Input renders in the middle row of a 3-row area (space / input / space)
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

fn draw_ticker(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let dim = Style::default().fg(theme.text_dim);
    let rows = area.height as usize;
    let start = app.ticker.len().saturating_sub(rows);
    let mut lines: Vec<Line> = Vec::new();

    // Pad at top so entries are flush against the bottom
    let entry_count = app.ticker.len().min(rows);
    for _ in 0..rows.saturating_sub(entry_count) {
        lines.push(Line::from(""));
    }

    for entry in app.ticker.iter().skip(start) {
        lines.push(Line::from(Span::styled(format!(" {entry}"), dim)));
    }

    let p = Paragraph::new(lines);
    f.render_widget(p, area);
}
