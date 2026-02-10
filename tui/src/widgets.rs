//! Shared rendering widgets — text formatting, headers, statuslines, and layout helpers.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use crate::app::View;
use crate::theme::Theme;

// =============================================================================
// TEXT UTILITIES
// =============================================================================

/// Split a string at a character boundary.
fn split_at_char_boundary(s: &str, max_chars: usize) -> (&str, &str) {
    let mut char_count = 0;
    let mut byte_idx = 0;

    for (i, c) in s.char_indices() {
        if char_count >= max_chars {
            byte_idx = i;
            break;
        }
        char_count += 1;
        byte_idx = i + c.len_utf8();
    }

    if char_count < max_chars {
        (s, "")
    } else {
        (&s[..byte_idx], &s[byte_idx..])
    }
}

// =============================================================================
// DRAWING
// =============================================================================

/// Render a section subheader with a colored underline rule.
#[allow(dead_code)]
pub fn draw_subheader(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    title: &str,
    underline_color: Color,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let title_area = Rect::new(area.x, area.y, area.width, 1);
    let mut title_line = title.to_string();
    let title_width = Line::from(title).width();
    let area_width = area.width as usize;
    // WHY: pad title to full width so the background style covers the entire row
    if title_width < area_width {
        title_line.push_str(&" ".repeat(area_width - title_width));
    }
    let header = Paragraph::new(title_line).style(Style::default().fg(theme.text_primary));
    f.render_widget(header, title_area);

    if area.height < 2 {
        return;
    }

    let underline_area = Rect::new(area.x, area.y + 1, area.width, 1);
    let line = "─".repeat(area.width as usize);
    let underline = Paragraph::new(line).style(Style::default().fg(underline_color));
    f.render_widget(underline, underline_area);
}

// =============================================================================
// HEADER & STATUSLINE
// =============================================================================

/// Render a 3-row view header with colored left/right border accents.
pub fn draw_header<'a>(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    title: impl Into<Line<'a>>,
    border_color: Color,
) {
    let bg_widget = Paragraph::new("").style(Style::default().bg(theme.header_bg));
    f.render_widget(bg_widget, area);

    let left_border =
        Paragraph::new("▎\n▎\n▎").style(Style::default().fg(border_color).bg(theme.header_bg));
    f.render_widget(left_border, Rect::new(area.x, area.y, 1, 3));

    let right_border =
        Paragraph::new("▕\n▕\n▕").style(Style::default().fg(border_color).bg(theme.header_bg));
    f.render_widget(
        right_border,
        Rect::new(area.x + area.width - 1, area.y, 1, 3),
    );

    let title_area = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1);
    let title_line: Line = title.into();
    let title_widget = Paragraph::new(title_line);
    f.render_widget(title_widget, title_area);
}

/// Render a single-row status bar with left and right content, matching header border style.
pub fn draw_statusline(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    left_content: Line,
    right_content: &str,
    border_color: Color,
) {
    let bg = theme.header_bg;

    let bg_widget = Paragraph::new("").style(Style::default().bg(bg));
    f.render_widget(bg_widget, area);

    let left_border = Paragraph::new("▎").style(Style::default().fg(border_color).bg(bg));
    f.render_widget(left_border, Rect::new(area.x, area.y, 1, 1));

    let right_border = Paragraph::new("▕").style(Style::default().fg(border_color).bg(bg));
    f.render_widget(
        right_border,
        Rect::new(area.x + area.width - 1, area.y, 1, 1),
    );

    let inner = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), 1);

    let right_width = Line::from(right_content).width() as u16;
    let right_width = right_width.min(inner.width);
    let right_x = inner.x + inner.width.saturating_sub(right_width);
    let left_width = right_x.saturating_sub(inner.x);

    if left_width > 0 {
        let left_area = Rect::new(inner.x, area.y, left_width, 1);
        f.render_widget(Paragraph::new(left_content), left_area);
    }

    if right_width > 0 {
        let right_area = Rect::new(right_x, area.y, right_width, 1);
        f.render_widget(
            Paragraph::new(right_content).style(Style::default().bg(bg).fg(theme.text_primary)),
            right_area,
        );
    }
}

// =============================================================================
// LAYOUT HELPERS
// =============================================================================

/// Create a centered rectangle occupying the given percentage of the parent area.
///
/// Used for popup/dialog overlays (view picker, confirmation dialogs).
pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    // WHY: three-chunk split on each axis — equal margins on both sides with content in the middle
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

/// Map a frame operation name to its display color.
pub fn op_color(op: &str) -> Color {
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

/// Truncate a string from the right, appending "..." when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }

    if s.chars().count() <= max {
        return s.to_string();
    }

    if max <= 3 {
        return s.chars().take(max).collect();
    }

    let (chunk, _) = split_at_char_boundary(s, max.saturating_sub(3));
    format!("{}...", chunk)
}

// =============================================================================
// TOP NAV & VIEW PICKER
// =============================================================================

/// Render a single-row top navigation bar with view tabs and status indicators.
#[allow(clippy::too_many_arguments)]
pub fn draw_top_nav(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: View,
    paused: bool,
    queued_count: usize,
    tick_count: usize,
    connected: bool,
) {
    let tabs: &[(&str, &str, View)] = &[("C", "Chat", View::Chat), ("M", "Monitor", View::Monitor)];

    let mut spans: Vec<Span> = Vec::new();
    for &(key, label, v) in tabs {
        let is_active = v == view;
        let text = format!(" [{}]{} ", key, label);
        let style = if is_active {
            Style::default().fg(theme.text_primary).bg(theme.header_bg)
        } else {
            Style::default().fg(theme.text_dim)
        };
        spans.push(Span::styled(text, style));
    }

    let left_line = Line::from(spans);
    let left_width = left_line.width() as u16;

    if left_width < area.width {
        let left_area = Rect::new(area.x, area.y, left_width, 1);
        f.render_widget(Paragraph::new(left_line), left_area);
    } else {
        f.render_widget(Paragraph::new(left_line), area);
    }

    // Right side: pause/queued/tick/connection indicators
    let (dot, dot_color) = if connected {
        ("●", theme.border_green)
    } else {
        ("●", theme.border_red)
    };

    let mut right_parts: Vec<Span> = Vec::new();
    if paused {
        right_parts.push(Span::styled(
            " PAUSED ",
            Style::default().fg(theme.border_yellow),
        ));
        if queued_count > 0 {
            right_parts.push(Span::styled(
                format!("+{} ", queued_count),
                Style::default().fg(theme.text_dim),
            ));
        }
    }
    right_parts.push(Span::styled(
        format!("t:{} ", tick_count),
        Style::default().fg(theme.text_dim),
    ));
    right_parts.push(Span::styled(
        format!("{} ", dot),
        Style::default().fg(dot_color),
    ));

    let right_line = Line::from(right_parts);
    let right_width = right_line.width() as u16;
    if area.width > right_width {
        let right_area = Rect::new(area.x + area.width - right_width, area.y, right_width, 1);
        f.render_widget(Paragraph::new(right_line), right_area);
    }
}

/// Render a centered popup for view selection.
pub fn draw_view_picker(f: &mut Frame, theme: &Theme, selected: usize) {
    let area = centered_rect(30, 20, f.area());
    f.render_widget(Clear, area);

    let items = ["Chat", "Monitor"];
    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let style = if i == selected {
                Style::default().fg(theme.selection)
            } else {
                Style::default().fg(theme.text_primary)
            };
            let marker = if i == selected { "● " } else { "  " };
            ListItem::new(format!("{}{}", marker, label)).style(style)
        })
        .collect();

    let list = List::new(list_items).block(
        Block::default()
            .title(" Views ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border_cyan)),
    );

    f.render_widget(list, area);
}
