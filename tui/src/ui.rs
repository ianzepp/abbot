//! Top-level UI layout — room tabs + active room + status bar.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, Mode};

/// Main draw dispatch.
pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // room tabs
            Constraint::Min(3),    // transcript + input
            Constraint::Length(1), // status bar
        ])
        .split(f.area());

    draw_tabs(f, app, chunks[0]);
    crate::room::draw_room(f, app, chunks[1]);
    draw_status(f, app, chunks[2]);
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let mut spans: Vec<Span> = Vec::new();
    for (i, room) in app.rooms.iter().enumerate() {
        let is_active = i == app.active_room;
        let label = format!("#{}", room.room);

        let unread_marker = if room.unread && !is_active { "*" } else { "" };
        let text = format!(" [{}]{}{} ", i + 1, label, unread_marker);

        let style = if is_active {
            Style::default().fg(theme.text_primary).bg(theme.header_bg)
        } else {
            Style::default().fg(theme.text_dim)
        };
        spans.push(Span::styled(text, style));
    }

    // Right-aligned connection indicator
    let (dot, dot_color) = if app.connected {
        ("●", theme.border_green)
    } else {
        ("●", theme.border_red)
    };

    let left_line = Line::from(spans);
    let left_width = left_line.width() as u16;

    if left_width < area.width {
        let left_area = Rect::new(area.x, area.y, left_width, 1);
        f.render_widget(Paragraph::new(left_line), left_area);
    } else {
        f.render_widget(Paragraph::new(left_line), area);
    }

    // Clock + connection dot on the right
    let clock = chrono::Local::now().format("%H:%M").to_string();
    let right_line = Line::from(vec![
        Span::styled(format!(" {} ", clock), Style::default().fg(theme.text_dim)),
        Span::styled(format!("{} ", dot), Style::default().fg(dot_color)),
    ]);
    let right_width = (clock.len() + 4) as u16; // " HH:MM " + "● "
    if area.width > right_width {
        let right_area = Rect::new(area.x + area.width - right_width, area.y, right_width, 1);
        f.render_widget(Paragraph::new(right_line), right_area);
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let room = app.current_room();

    let room_label = format!("#{}", room.room);
    let msg_count = room.messages.len();
    let mode_label = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
    };

    let pending = if room.pending { " [...]" } else { "" };

    // Truncate CWD from the left if too long
    let max_cwd = 30;
    let cwd_display = if app.cwd.chars().count() > max_cwd {
        let tail: String = app.cwd.chars().rev().take(max_cwd - 3).collect();
        let tail: String = tail.chars().rev().collect();
        format!("...{}", tail)
    } else {
        app.cwd.clone()
    };

    let left = Line::from(vec![
        Span::styled(
            format!(" {} ", room_label),
            Style::default().fg(theme.accent).bg(theme.header_bg),
        ),
        Span::styled(
            format!(" {} msgs{} ", msg_count, pending),
            Style::default().fg(theme.text_dim).bg(theme.header_bg),
        ),
        Span::styled(
            format!(" {} ", mode_label),
            Style::default().fg(theme.text_primary).bg(theme.header_bg),
        ),
        Span::styled(
            format!(" {} ", cwd_display),
            Style::default().fg(theme.text_dim).bg(theme.header_bg),
        ),
    ]);

    let right_text = match app.mode {
        Mode::Normal => "i:type  q:quit",
        Mode::Insert => "Enter:send  /:cmd  !:bash  Esc:normal",
    };

    let bg = Paragraph::new("").style(Style::default().bg(theme.header_bg));
    f.render_widget(bg, area);

    let right_width = right_text.len() as u16 + 1;
    let left_width = area.width.saturating_sub(right_width);

    if left_width > 0 {
        let left_area = Rect::new(area.x, area.y, left_width, 1);
        f.render_widget(Paragraph::new(left), left_area);
    }

    if right_width > 0 && area.width > right_width {
        let right_area = Rect::new(area.x + area.width - right_width, area.y, right_width, 1);
        f.render_widget(
            Paragraph::new(right_text)
                .style(Style::default().fg(theme.text_dim).bg(theme.header_bg)),
            right_area,
        );
    }
}
