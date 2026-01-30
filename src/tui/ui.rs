use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::bus::{MessageData, MessageOp, Origin};

use super::App;

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(3),    // messages
        Constraint::Length(3), // input
        Constraint::Length(1), // hotkeys
    ])
    .split(frame.area());

    draw_header(frame, chunks[0], app);
    draw_messages(frame, chunks[1], app);
    draw_input(frame, chunks[2], app);
    draw_hotkeys(frame, chunks[3]);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let scope = app.scope().to_string();
    let header = Paragraph::new(Line::from(vec![Span::styled(
        scope,
        Style::default().fg(Color::Cyan).bold(),
    )]));
    frame.render_widget(header, area);
}

fn draw_messages(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::TOP | Borders::BOTTOM);
    let inner = block.inner(area);

    let visible_height = inner.height as usize;
    let messages = app.messages();
    let total = messages.len();
    let offset = app.scroll_offset();

    let start = total.saturating_sub(visible_height + offset);
    let end = total.saturating_sub(offset);

    let lines: Vec<Line> = messages[start..end]
        .iter()
        .map(|msg| format_message(msg))
        .collect();

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, area);
}

fn draw_input(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::TOP);
    let input_text = format!("> {}_", app.input());
    let para = Paragraph::new(input_text).block(block);
    frame.render_widget(para, area);
}

fn draw_hotkeys(frame: &mut Frame, area: Rect) {
    let hotkeys = Paragraph::new(Line::from(vec![
        Span::styled("^C", Style::default().fg(Color::Yellow)),
        Span::raw(" quit"),
    ]))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hotkeys, area);
}

fn format_message(msg: &crate::bus::Message) -> Line<'static> {
    let sender_color = match msg.origin {
        Origin::Human => Color::Green,
        Origin::Head => Color::Cyan,
        Origin::Hand => Color::Yellow,
        Origin::System => Color::DarkGray,
    };

    let sender = Span::styled(
        format!("[{}] ", msg.sender),
        Style::default().fg(sender_color),
    );

    let content = match (&msg.op, &msg.data) {
        (MessageOp::Chat, MessageData::Text(t)) => t.clone(),
        (MessageOp::Task, MessageData::Task(task)) => format_task(task),
        (MessageOp::Ping, _) => return Line::from(""),
        (op, data) => format!("{:?}: {:?}", op, data),
    };

    Line::from(vec![sender, Span::raw(content)])
}

fn format_task(task: &crate::bus::TaskMsg) -> String {
    use crate::bus::TaskMsg;
    match task {
        TaskMsg::Request { task_id, goal, .. } => format!("task {} requested: {}", task_id, goal),
        TaskMsg::Assigned {
            task_id, hand_id, ..
        } => format!("task {} -> {}", task_id, hand_id),
        TaskMsg::Progress { task_id, note, .. } => format!("task {} progress: {}", task_id, note),
        TaskMsg::Echo {
            task_id,
            tool,
            content,
            ..
        } => {
            format!("task {} [{}]: {}", task_id, tool, truncate(content, 60))
        }
        TaskMsg::Result {
            task_id,
            ok,
            summary,
            ..
        } => {
            let status = if *ok { "OK" } else { "FAILED" };
            format!("task {} {}: {}", task_id, status, truncate(summary, 60))
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
