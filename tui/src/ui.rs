//! Top-level UI layout — room tabs + active room + status bar.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, AppView, Mode};

/// Returns visible tabs based on developer mode.
fn visible_tabs(developer: bool) -> Vec<(char, &'static str, AppView)> {
    let mut tabs = vec![('1', "Main", AppView::Chat), ('2', "Hands", AppView::Hands)];
    if developer {
        tabs.push(('3', "Frames", AppView::Frames));
        tabs.push(('4', "EMS", AppView::Ems));
    }
    tabs
}

/// Main draw dispatch.
pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // content area
            Constraint::Length(1), // global menubar
        ])
        .split(f.area());

    match app.active_view {
        AppView::Chat | AppView::Hands => crate::room::draw_room(f, app, chunks[0]),
        AppView::Frames => draw_stub(f, app, chunks[0], "Frames"),
        AppView::Ems => draw_stub(f, app, chunks[0], "EMS"),
    }

    draw_tabs(f, app, chunks[1]);
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    // Fill full-width background (same pattern as footer)
    let bg = Paragraph::new("").style(Style::default().bg(theme.header_bg));
    f.render_widget(bg, area);

    // Left side: static tabs
    let tabs = visible_tabs(app.developer);
    let mut spans: Vec<Span> = Vec::new();
    for &(key, label, view) in &tabs {
        let is_active = app.active_view == view;
        let style = if is_active {
            Style::default().fg(theme.text_primary).bg(theme.header_bg)
        } else {
            Style::default().fg(theme.text_dim).bg(theme.header_bg)
        };
        spans.push(Span::styled(format!(" [{}] {} ", key, label), style));
    }

    let left_line = Line::from(spans);
    let left_width = left_line.width() as u16;
    if left_width < area.width {
        let left_area = Rect::new(area.x, area.y, left_width, 1);
        f.render_widget(Paragraph::new(left_line), left_area);
    } else {
        f.render_widget(Paragraph::new(left_line), area);
    }

    // Right side: cwd  branch  ● [seq:#] [HH:MM]
    let clock = chrono::Local::now().format("%H:%M").to_string();
    let conn_color = if app.connected {
        theme.border_green
    } else {
        theme.border_red
    };
    let dim_bg = Style::default().fg(theme.text_dim).bg(theme.header_bg);

    let mut right_spans = vec![Span::styled(format!(" {} ", app.cwd), dim_bg)];
    if !app.git_branch.is_empty() {
        right_spans.push(Span::styled(format!(" {} ", app.git_branch), dim_bg));
    }
    right_spans.push(Span::styled(
        "\u{25CF} ",
        Style::default().fg(conn_color).bg(theme.header_bg),
    ));
    right_spans.push(Span::styled(format!("[seq:{}] ", app.daemon_seq), dim_bg));
    right_spans.push(Span::styled(format!("[{}] ", clock), dim_bg));
    let right_line = Line::from(right_spans);
    let right_width = right_line.width() as u16;
    if area.width > right_width {
        let right_area = Rect::new(area.x + area.width - right_width, area.y, right_width, 1);
        f.render_widget(Paragraph::new(right_line), right_area);
    }
}

pub fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let room = app.current_room();

    let room_label = format!("#{}", room.room);
    let msg_count = room.messages.len();
    let mode_label = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
    };

    let pending = if let Some(ref st) = room.status_text {
        format!(" {}", st)
    } else if room.pending {
        " [...]".to_string()
    } else {
        String::new()
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

fn draw_stub(f: &mut Frame, app: &App, area: Rect, label: &str) {
    let theme = &app.theme;
    let text = format!("  {} (coming soon)", label);
    let widget = Paragraph::new(text).style(Style::default().fg(theme.text_dim));
    f.render_widget(widget, area);
}
