use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use crate::View;
use crate::theme::Theme;

pub fn ellipsize_left(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }

    let len = s.chars().count();
    if len <= max {
        return s.to_string();
    }

    if max <= 3 {
        return s.chars().take(max).collect();
    }

    let take = max.saturating_sub(3);
    let mut start = s.len();
    let mut seen = 0usize;
    for (i, _) in s.char_indices().rev() {
        seen += 1;
        if seen == take {
            start = i;
            break;
        }
        start = i;
    }

    format!("...{}", &s[start..])
}

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

/// Wrap text to fit within a given width, with indentation on continuation lines.
/// Returns a vector of lines with owned content.
#[allow(dead_code)]
pub fn wrap_text(text: &str, width: usize, indent: usize, base_style: Style) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![];
    }

    let mut result: Vec<Line<'static>> = Vec::new();
    let indent_str: String = " ".repeat(indent);

    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            result.push(Line::from(""));
            continue;
        }

        let mut current_line = String::new();
        let mut is_first_line = true;

        for word in paragraph.split_whitespace() {
            let effective_width = if is_first_line {
                width
            } else {
                width.saturating_sub(indent)
            };
            let word_len = word.chars().count();
            let current_len = current_line.chars().count();
            let space_needed = if current_line.is_empty() { 0 } else { 1 };

            if current_len + space_needed + word_len <= effective_width {
                if !current_line.is_empty() {
                    current_line.push(' ');
                }
                current_line.push_str(word);
            } else {
                if !current_line.is_empty() {
                    let line_text = if is_first_line {
                        current_line.clone()
                    } else {
                        format!("{}{}", indent_str, current_line)
                    };
                    result.push(Line::from(Span::styled(line_text, base_style)));
                    is_first_line = false;
                }
                current_line = word.to_string();
            }
        }

        if !current_line.is_empty() {
            let line_text = if is_first_line {
                current_line
            } else {
                format!("{}{}", indent_str, current_line)
            };
            result.push(Line::from(Span::styled(line_text, base_style)));
        }
    }

    if result.is_empty() {
        result.push(Line::from(""));
    }

    result
}

/// Convert markdown text to styled spans for terminal display.
/// Supports: **bold**, *italic*, `code`, and # headers.
/// Returns owned spans (with 'static lifetime).
pub fn markdown_to_spans(text: &str, base_style: Style, code_color: Color) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut current_text = String::new();
    let mut at_line_start = true;

    while let Some((_i, c)) = chars.next() {
        match c {
            '\n' => {
                current_text.push('\n');
                at_line_start = true;
            }
            '`' => {
                if !current_text.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut current_text), base_style));
                }
                let mut code_content = String::new();
                for (_, ch) in chars.by_ref() {
                    if ch == '`' {
                        break;
                    }
                    code_content.push(ch);
                }
                if !code_content.is_empty() {
                    spans.push(Span::styled(code_content, Style::default().fg(code_color)));
                }
                at_line_start = false;
            }
            '*' => {
                if chars.peek().map(|(_, ch)| *ch == '*').unwrap_or(false) {
                    chars.next();
                    if !current_text.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut current_text), base_style));
                    }
                    let mut bold_content = String::new();
                    while let Some((_, ch)) = chars.next() {
                        if ch == '*' && chars.peek().map(|(_, c)| *c == '*').unwrap_or(false) {
                            chars.next();
                            break;
                        }
                        bold_content.push(ch);
                    }
                    if !bold_content.is_empty() {
                        spans.push(Span::styled(
                            bold_content,
                            base_style.add_modifier(Modifier::BOLD),
                        ));
                    }
                } else {
                    if !current_text.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut current_text), base_style));
                    }
                    let mut italic_content = String::new();
                    for (_, ch) in chars.by_ref() {
                        if ch == '*' {
                            break;
                        }
                        italic_content.push(ch);
                    }
                    if !italic_content.is_empty() {
                        spans.push(Span::styled(
                            italic_content,
                            base_style.add_modifier(Modifier::ITALIC),
                        ));
                    }
                }
                at_line_start = false;
            }
            '#' if at_line_start => {
                if !current_text.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut current_text), base_style));
                }
                while chars
                    .peek()
                    .map(|(_, ch)| *ch == '#' || *ch == ' ')
                    .unwrap_or(false)
                {
                    chars.next();
                }
                let mut header_content = String::new();
                let mut had_newline = false;
                for (_, ch) in chars.by_ref() {
                    if ch == '\n' {
                        had_newline = true;
                        break;
                    }
                    header_content.push(ch);
                }
                if !header_content.is_empty() {
                    spans.push(Span::styled(
                        header_content,
                        base_style.add_modifier(Modifier::BOLD),
                    ));
                }
                if had_newline {
                    current_text.push('\n');
                }
                at_line_start = true;
            }
            _ => {
                current_text.push(c);
                at_line_start = false;
            }
        }
    }

    if !current_text.is_empty() {
        spans.push(Span::styled(current_text, base_style));
    }

    spans
}

/// Format a chat message with proper alignment and wrapping.
/// Returns multiple lines for wrapped content.
#[allow(clippy::too_many_arguments)]
pub fn format_chat_message(
    time: &str,
    nick: &str,
    content: &str,
    time_style: Style,
    nick_style: Style,
    content_style: Style,
    code_color: Color,
    width: usize,
    use_markdown: bool,
) -> Vec<Line<'static>> {
    // Format: "09:15   <nick> content"
    // Time is 5 chars, then spaces, then right-aligned <nick>, then space
    const NICK_WIDTH: usize = 10;
    let nick_with_brackets = format!("<{}>", nick);
    let nick_padded = format!("{:>width$}", nick_with_brackets, width = NICK_WIDTH);
    let prefix = format!("{}  {} ", time, nick_padded);
    let prefix_width = prefix.chars().count();

    if width <= prefix_width {
        return vec![Line::from(vec![Span::styled(
            format!(
                "{}  {:>width$} ",
                time,
                nick_with_brackets,
                width = NICK_WIDTH
            ),
            time_style,
        )])];
    }

    let content_width = width - prefix_width;
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Process content line by line for proper wrapping
    for (para_idx, paragraph) in content.split('\n').enumerate() {
        if paragraph.is_empty() {
            if para_idx == 0 {
                // Compute padding for right-alignment
                let pad = NICK_WIDTH.saturating_sub(nick_with_brackets.chars().count());
                lines.push(Line::from(vec![
                    Span::styled(format!("{}  {}", time, " ".repeat(pad)), time_style),
                    Span::styled(String::from("<"), time_style),
                    Span::styled(nick.to_string(), nick_style),
                    Span::styled(String::from("> "), time_style),
                ]));
            } else {
                lines.push(Line::from(""));
            }
            continue;
        }

        // Wrap this paragraph
        let wrapped = wrap_paragraph(paragraph, content_width);

        for (line_idx, line_text) in wrapped.into_iter().enumerate() {
            let is_first = para_idx == 0 && line_idx == 0;

            if is_first {
                // Compute padding for right-alignment
                let pad = NICK_WIDTH.saturating_sub(nick_with_brackets.chars().count());
                let mut spans: Vec<Span<'static>> = vec![
                    Span::styled(format!("{}  {}", time, " ".repeat(pad)), time_style),
                    Span::styled(String::from("<"), time_style),
                    Span::styled(nick.to_string(), nick_style),
                    Span::styled(String::from("> "), time_style),
                ];

                if use_markdown {
                    spans.extend(markdown_to_spans(&line_text, content_style, code_color));
                } else {
                    spans.push(Span::styled(line_text, content_style));
                }
                lines.push(Line::from(spans));
            } else {
                let indent = " ".repeat(prefix_width);
                let mut spans: Vec<Span<'static>> = vec![Span::styled(indent, Style::default())];

                if use_markdown {
                    spans.extend(markdown_to_spans(&line_text, content_style, code_color));
                } else {
                    spans.push(Span::styled(line_text, content_style));
                }
                lines.push(Line::from(spans));
            }
        }
    }

    if lines.is_empty() {
        let pad = NICK_WIDTH.saturating_sub(nick_with_brackets.chars().count());
        lines.push(Line::from(vec![
            Span::styled(format!("{}  {}", time, " ".repeat(pad)), time_style),
            Span::styled(String::from("<"), time_style),
            Span::styled(nick.to_string(), nick_style),
            Span::styled(String::from("> "), time_style),
        ]));
    }

    lines
}

/// Wrap a single paragraph to fit within width.
fn wrap_paragraph(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        let current_len = current_line.chars().count();
        let space_needed = if current_line.is_empty() { 0 } else { 1 };

        if current_len + space_needed + word_len <= width {
            if !current_line.is_empty() {
                current_line.push(' ');
            }
            current_line.push_str(word);
        } else {
            if !current_line.is_empty() {
                lines.push(std::mem::take(&mut current_line));
            }
            // Handle words longer than width
            if word_len > width {
                let mut remaining = word;
                while remaining.chars().count() > width {
                    let (chunk, rest) = split_at_char_boundary(remaining, width);
                    lines.push(chunk.to_string());
                    remaining = rest;
                }
                current_line = remaining.to_string();
            } else {
                current_line = word.to_string();
            }
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

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

pub fn draw_header_with_right<'a>(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    left: impl Into<Line<'a>>,
    right: &str,
    right_style: Style,
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

    let right_width = Line::from(right).width() as u16;
    let right_width = right_width.min(title_area.width);
    let right_x = title_area.x + title_area.width.saturating_sub(right_width);
    let left_width = right_x.saturating_sub(title_area.x);

    if left_width > 0 {
        let left_area = Rect::new(title_area.x, title_area.y, left_width, 1);
        let left_widget = Paragraph::new(left.into());
        f.render_widget(left_widget, left_area);
    }

    if right_width > 0 {
        let right_area = Rect::new(right_x, title_area.y, right_width, 1);
        let right_widget = Paragraph::new(right).style(right_style);
        f.render_widget(right_widget, right_area);
    }
}

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

#[allow(clippy::too_many_arguments)]
pub fn draw_top_nav(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    current_view: View,
    paused: bool,
    queued_count: usize,
    tick_count: usize,
    connected: bool,
) {
    let items = [
        ("1", "Chat", View::Chat),
        ("2", "Monitor", View::Monitor),
        ("3", "Explorer", View::Explorer),
        ("4", "Config", View::Config),
        ("5", "Logs", View::Logs),
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

    let left_line = Line::from(spans);

    let (status_text, status_color) = if connected {
        ("●", theme.border_green)
    } else {
        ("●", theme.border_red)
    };

    let mut right_spans: Vec<Span> = Vec::new();

    if paused {
        right_spans.push(Span::styled(
            "[PAUSED] ",
            Style::default().fg(theme.border_red),
        ));
        if queued_count > 0 {
            right_spans.push(Span::styled(
                format!("[q:{}] ", queued_count),
                Style::default().fg(theme.border_yellow),
            ));
        }
    }

    right_spans.push(Span::styled(
        format!("[t:{}] ", tick_count),
        Style::default().fg(theme.text_dim),
    ));

    let time = chrono::Local::now().format("%H:%M");
    right_spans.push(Span::styled(
        format!("{} ", time),
        Style::default().fg(theme.text_dim),
    ));

    right_spans.push(Span::styled(status_text, Style::default().fg(status_color)));

    let right_line = Line::from(right_spans);
    let right_width = (right_line.width() as u16).min(area.width);
    let right_x = area.x + area.width.saturating_sub(right_width);
    let left_width = right_x.saturating_sub(area.x);

    if left_width > 0 {
        let left_area = Rect::new(area.x, area.y, left_width, 1);
        f.render_widget(Paragraph::new(left_line), left_area);
    }

    if right_width > 0 {
        let right_area = Rect::new(right_x, area.y, right_width, 1);
        f.render_widget(Paragraph::new(right_line), right_area);
    }
}

pub fn draw_view_picker(f: &mut Frame, theme: &Theme, view_picker_selected: usize) {
    let area = centered_rect(30, 30, f.area());
    f.render_widget(Clear, area);

    let views = ["Chat", "Monitor", "Explorer", "Config", "Logs"];

    let items: Vec<ListItem> = views
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let marker = if i == view_picker_selected {
                "● "
            } else {
                "  "
            };
            let style = if i == view_picker_selected {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.text_dim)
            };
            ListItem::new(format!(" {} {}", marker, name)).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Switch View ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border_cyan))
            .padding(ratatui::widgets::Padding::uniform(1)),
    );

    f.render_widget(list, area);
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
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
