use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::widgets::{draw_header, draw_statusline, draw_top_nav, draw_view_picker};
use crate::App;

pub fn draw_chat(f: &mut Frame, app: &App) {
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
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(h_chunks[1]);

    draw_top_nav(f, &app.theme, chunks[1], app.view, app.paused, app.queued_count, app.tick_count, app.connected);
    draw_chat_header(f, app, chunks[3]);

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

    if app.chat_insert_mode {
        let cursor_x = input_area.x + mode_indicator.len() as u16 + prompt.len() as u16 + app.compose_input.visual_cursor() as u16;
        f.set_cursor_position((cursor_x, input_area.y));
    }

    draw_chat_status(f, app, chunks[7]);

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }
}

fn draw_chat_header(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    draw_header(f, theme, area, " Chat #main", theme.border_blue);
}

fn draw_chat_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let mode_text = if app.chat_insert_mode { "INSERT" } else { "NORMAL" };

    let left = Line::from(vec![
        Span::styled("[#main]", Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [msgs:{}]", app.chat_messages.len()), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [{}]", mode_text), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
    ]);

    draw_statusline(f, theme, area, left, "[^T] [^C]", theme.border_blue);
}
