use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::widgets::{
    draw_header, draw_statusline, draw_top_nav, draw_view_picker, format_chat_message,
};
use crate::{App, ChatMode};

pub fn draw_chat(f: &mut Frame, app: &App) {
    if app.chat_mode == ChatMode::ScopePicker {
        draw_scope_picker(f, app);
        return;
    }

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

    if app.chat_history_loading {
        let loading =
            Paragraph::new("Loading chat history...").style(Style::default().fg(theme.text_dim));
        f.render_widget(loading, messages_area);
    } else if app.chat_messages.is_empty() {
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
    if app.chat_mode == ChatMode::Insert {
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
        let hint = Paragraph::new("Press [i] to type  [s] scope picker")
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(hint, input_area);
    }

    draw_chat_status(f, app, chunks[8]);

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }
}

fn draw_scope_picker(f: &mut Frame, app: &App) {
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
            Constraint::Min(5),    // scope list
            Constraint::Length(1), // hint
            Constraint::Length(1), // margin
            Constraint::Length(1), // status bar
        ])
        .split(h_chunks[1]);

    let theme = &app.theme;

    draw_top_nav(
        f,
        theme,
        chunks[1],
        app.view,
        app.paused,
        app.queued_count,
        app.tick_count,
        app.connected,
    );
    draw_header(f, theme, chunks[3], " Select Scope", theme.border_blue);

    let list_area = chunks[5];

    if app.scope_loading {
        let loading =
            Paragraph::new("  Loading scopes...").style(Style::default().fg(theme.text_dim));
        f.render_widget(loading, list_area);
    } else if let Some(ref err) = app.scope_error {
        let error = Paragraph::new(format!("  Error: {}", err))
            .style(Style::default().fg(theme.border_red));
        f.render_widget(error, list_area);
    } else if app.scope_entries.is_empty() {
        let empty = Paragraph::new("  No scopes found. Press Enter to use default (#main).")
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(empty, list_area);
    } else {
        let visible = list_area.height as usize;
        let offset = if app.scope_selected >= visible {
            app.scope_selected - visible + 1
        } else {
            0
        };

        let mut lines: Vec<Line> = Vec::new();
        for (i, entry) in app
            .scope_entries
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
        {
            let selected = i == app.scope_selected;
            let marker = if selected { "> " } else { "  " };
            let label = format!(
                "{}#{:<24} {:>6} frames",
                marker, entry.scope, entry.frame_count
            );
            let style = if selected {
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_secondary)
            };
            lines.push(Line::from(Span::styled(label, style)));
        }

        let list = Paragraph::new(lines);
        f.render_widget(list, list_area);
    }

    let hint_area = chunks[6];
    let hint = Paragraph::new("[Enter] select  [r] refresh  [^T] switch view")
        .style(Style::default().fg(theme.text_dim));
    f.render_widget(hint, hint_area);

    let status_left = Line::from(vec![Span::styled(
        format!(" [{} scopes]", app.scope_entries.len()),
        Style::default().bg(theme.header_bg).fg(theme.text_primary),
    )]);
    draw_statusline(
        f,
        theme,
        chunks[8],
        status_left,
        "[^T] [^C]",
        theme.border_blue,
    );

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }
}

fn draw_chat_header(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let scope = app.chat_scope.as_deref().unwrap_or("main");
    let title = format!(" Chat #{}", scope);
    draw_header(f, theme, area, title.as_str(), theme.border_blue);
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
    let scope = app.chat_scope.as_deref().unwrap_or("main");

    let mode_text = match app.chat_mode {
        ChatMode::Insert => "INSERT",
        ChatMode::Normal => "NORMAL",
        ChatMode::ScopePicker => "SCOPE",
    };

    let left = Line::from(vec![
        Span::styled(
            format!("[#{}]", scope),
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

    let right = if app.chat_mode == ChatMode::Normal {
        "[s] scope  [^T] [^C]"
    } else {
        "[^T] [^C]"
    };

    draw_statusline(f, theme, area, left, right, theme.border_blue);
}
