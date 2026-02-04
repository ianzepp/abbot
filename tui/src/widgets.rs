use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use crate::theme::Theme;
use crate::View;

pub fn draw_header<'a>(f: &mut Frame, theme: &Theme, area: Rect, title: impl Into<Line<'a>>, border_color: Color) {
    let bg_widget = Paragraph::new("").style(Style::default().bg(theme.header_bg));
    f.render_widget(bg_widget, area);

    let left_border = Paragraph::new("▎\n▎\n▎")
        .style(Style::default().fg(border_color).bg(theme.header_bg));
    f.render_widget(left_border, Rect::new(area.x, area.y, 1, 3));

    let right_border = Paragraph::new("▕\n▕\n▕")
        .style(Style::default().fg(border_color).bg(theme.header_bg));
    f.render_widget(right_border, Rect::new(area.x + area.width - 1, area.y, 1, 3));

    let title_area = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1);
    let title_line: Line = title.into();
    let title_widget = Paragraph::new(title_line);
    f.render_widget(title_widget, title_area);
}

pub fn draw_statusline(f: &mut Frame, theme: &Theme, area: Rect, left_content: Line, right_content: &str, border_color: Color) {
    let bg = theme.header_bg;

    let bg_widget = Paragraph::new("").style(Style::default().bg(bg));
    f.render_widget(bg_widget, area);

    let left_border = Paragraph::new("▎").style(Style::default().fg(border_color).bg(bg));
    f.render_widget(left_border, Rect::new(area.x, area.y, 1, 1));

    let right_border = Paragraph::new("▕").style(Style::default().fg(border_color).bg(bg));
    f.render_widget(right_border, Rect::new(area.x + area.width - 1, area.y, 1, 1));

    let left_area = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), 1);
    f.render_widget(Paragraph::new(left_content), left_area);

    let right_area = Rect::new(
        area.x + area.width.saturating_sub(right_content.len() as u16 + 2),
        area.y,
        right_content.len() as u16,
        1,
    );
    f.render_widget(Paragraph::new(right_content).style(Style::default().bg(bg).fg(theme.text_primary)), right_area);
}

pub fn draw_top_nav(f: &mut Frame, theme: &Theme, area: Rect, current_view: View, paused: bool, queued_count: usize, tick_count: usize, connected: bool, dark_mode: bool) {
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

    let (status_text, status_color) = if connected {
        ("●", theme.border_green)
    } else {
        ("●", theme.border_red)
    };

    let mut right_spans: Vec<Span> = Vec::new();

    if paused {
        right_spans.push(Span::styled("[PAUSED] ", Style::default().fg(theme.border_red)));
        if queued_count > 0 {
            right_spans.push(Span::styled(format!("[q:{}] ", queued_count), Style::default().fg(theme.border_yellow)));
        }
    }

    right_spans.push(Span::styled(format!("[t:{}] ", tick_count), Style::default().fg(theme.text_dim)));
    right_spans.push(Span::styled(status_text, Style::default().fg(status_color)));

    let theme_icon = if dark_mode { " ☾" } else { " ☀" };
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

pub fn draw_view_picker(f: &mut Frame, theme: &Theme, view_picker_selected: usize) {
    let area = centered_rect(30, 30, f.area());
    f.render_widget(Clear, area);

    let views = ["Monitor", "Chat", "Explorer", "Config"];

    let items: Vec<ListItem> = views
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let marker = if i == view_picker_selected { "● " } else { "  " };
            let style = if i == view_picker_selected {
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
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
