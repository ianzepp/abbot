use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::widgets::{
    draw_header, draw_statusline, draw_top_nav, draw_view_picker, format_chat_message,
};
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
            Constraint::Length(1), // top margin
            Constraint::Length(1), // top nav
            Constraint::Length(1), // margin
            Constraint::Length(3), // header
            Constraint::Length(1), // margin
            Constraint::Min(5),    // messages
            Constraint::Length(1), // input
            Constraint::Length(1), // margin
            Constraint::Length(1), // status bar
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
    draw_chat_header(f, app, chunks[3]);

    let theme = &app.theme;
    let messages_area = chunks[5];
    let visible_lines = messages_area.height as usize;
    let content_width = messages_area.width as usize;

    if app.chat_messages.is_empty() {
        draw_chat_splash(f, app, messages_area);
    } else {
        let mut lines: Vec<Line> = Vec::new();
        for msg in &app.chat_messages {
            let time = msg.timestamp.format("%H:%M").to_string();
            let (nick, nick_style, content_style, use_markdown) = match msg.role.as_str() {
                "user" => (
                    "you",
                    Style::default().fg(theme.border_cyan),
                    Style::default().fg(theme.text_primary),
                    false,
                ),
                "error" => (
                    "error",
                    Style::default().fg(theme.border_red),
                    Style::default().fg(theme.border_red),
                    false,
                ),
                _ => (
                    "abbot",
                    Style::default().fg(theme.border_green),
                    Style::default().fg(theme.text_primary),
                    true,
                ),
            };

            let time_style = Style::default().fg(theme.text_dim);
            let msg_lines = format_chat_message(
                &time,
                nick,
                &msg.content,
                time_style,
                nick_style,
                content_style,
                theme.border_cyan,
                content_width,
                use_markdown,
            );
            lines.extend(msg_lines);
        }

        let scroll = if lines.len() > visible_lines {
            lines.len() - visible_lines
        } else {
            0
        };

        let messages = Paragraph::new(lines).scroll((scroll as u16, 0));
        f.render_widget(messages, messages_area);
    }

    let input_area = chunks[6];
    if app.chat_insert_mode {
        let input_line = Line::from(vec![
            Span::styled("INSERT ", Style::default().fg(theme.border_green)),
            Span::styled("> ", Style::default().fg(theme.text_dim)),
            Span::styled(
                app.compose_input.value(),
                Style::default().fg(theme.text_primary),
            ),
        ]);
        f.render_widget(Paragraph::new(input_line), input_area);

        let cursor_x = input_area.x
            + "INSERT ".len() as u16
            + "> ".len() as u16
            + app.compose_input.visual_cursor() as u16;
        f.set_cursor_position((cursor_x, input_area.y));
    } else {
        let hint = Paragraph::new("Press [i] to enable chat messaging")
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(hint, input_area);
    }

    draw_chat_status(f, app, chunks[8]);

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }
}

fn draw_chat_header(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    draw_header(f, theme, area, " Chat #main", theme.border_blue);
}

fn draw_chat_splash(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let body = Style::default().fg(theme.border_blue);
    let eyes = Style::default().fg(theme.text_primary);
    let dim = Style::default().fg(theme.text_dim);
    let version = env!("CARGO_PKG_VERSION");

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(" ▗▄███▄▖", body)),
        Line::from(vec![
            Span::styled("  █", body),
            Span::styled("◉ ◉", eyes),
            Span::styled("█", body),
        ]),
        Line::from(Span::styled("  ⠿ ⠿ ⠿", body)),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  Abbot",
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" v{}", version), dim),
        ]),
        Line::from(vec![
            Span::styled("  Head", Style::default().fg(theme.border_cyan)),
            Span::styled(" · ", dim),
            Span::styled("Hand", Style::default().fg(theme.border_green)),
            Span::styled(" · ", dim),
            Span::styled("Mind", Style::default().fg(theme.border_magenta)),
        ]),
        Line::from(""),
        Line::from(Span::styled("  Waiting for messages...", dim)),
    ];

    // Vertically center in the area
    let content_height = lines.len();
    let pad = (area.height as usize).saturating_sub(content_height) / 2;
    if pad > 0 {
        let padding: Vec<Line> = (0..pad).map(|_| Line::from("")).collect();
        lines.splice(0..0, padding);
    }

    let splash = Paragraph::new(lines);
    f.render_widget(splash, area);
}

fn draw_chat_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let mode_text = if app.chat_insert_mode {
        "INSERT"
    } else {
        "NORMAL"
    };

    let left = Line::from(vec![
        Span::styled(
            "[#main]",
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
        Span::styled(
            format!(" [msgs:{}]", app.chat_messages.len()),
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
        Span::styled(
            format!(" [{}]", mode_text),
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
    ]);

    draw_statusline(f, theme, area, left, "[^T] [^C]", theme.border_blue);
}
