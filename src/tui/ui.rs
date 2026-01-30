use std::time::SystemTime;

use chrono::{DateTime, Local};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::bus::{MessageData, MessageOp, Origin};

use super::app::TaskStatus;
use super::App;

const NICK_WIDTH: usize = 12;
const TIMESTAMP_WIDTH: usize = 5; // "HH:MM"
const PREFIX_WIDTH: usize = TIMESTAMP_WIDTH + 1 + NICK_WIDTH + 1; // "HH:MM nick "

const BG_MAIN: Color = Color::Reset;
const BG_TASKS: Color = Color::Rgb(30, 30, 40);

pub fn draw(frame: &mut Frame, app: &App) {
    let h_chunks = Layout::horizontal([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(frame.area());

    draw_main_panel(frame, h_chunks[0], app);
    draw_tasks_panel(frame, h_chunks[1], app);
}

fn draw_main_panel(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // topic bar
        Constraint::Min(1),    // messages
        Constraint::Length(1), // status bar
        Constraint::Length(1), // input line
    ])
    .split(area);

    draw_topic(frame, chunks[0], app);
    draw_messages(frame, chunks[1], app);
    draw_status_bar(frame, chunks[2], app);
    draw_input(frame, chunks[3], app);
}

fn draw_tasks_panel(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(1),    // task list
    ])
    .split(area);

    let header =
        Paragraph::new(" Tasks ").style(Style::default().bg(Color::DarkGray).fg(Color::White));
    frame.render_widget(header, chunks[0]);

    let tasks = app.tasks();
    let mut task_list: Vec<_> = tasks.values().collect();
    task_list.sort_by(|a, b| a.task_id.cmp(&b.task_id));

    let visible_height = chunks[1].height as usize;
    let width = chunks[1].width as usize;

    let lines: Vec<Line> = task_list
        .iter()
        .rev()
        .take(visible_height)
        .map(|task| format_task_line(task, width))
        .collect();

    let para = Paragraph::new(lines).style(Style::default().bg(BG_TASKS));
    frame.render_widget(para, chunks[1]);
}

fn format_task_line(task: &super::app::TaskInfo, width: usize) -> Line<'static> {
    let (status_char, status_color) = match &task.status {
        TaskStatus::Requested => ('?', Color::Yellow),
        TaskStatus::Assigned(_) => ('>', Color::Cyan),
        TaskStatus::InProgress(_) => ('*', Color::Blue),
        TaskStatus::Done(true, _) => ('+', Color::Green),
        TaskStatus::Done(false, _) => ('!', Color::Red),
    };

    let id_short = if task.task_id.len() > 8 {
        &task.task_id[..8]
    } else {
        &task.task_id
    };

    let status_info = match &task.status {
        TaskStatus::Requested => String::new(),
        TaskStatus::Assigned(h) => format!(" -> {}", h),
        TaskStatus::InProgress(note) => format!(" {}", truncate(note, 20)),
        TaskStatus::Done(_, summary) => format!(" {}", truncate(summary, 20)),
    };

    let goal_width = width.saturating_sub(12 + status_info.len());
    let goal = truncate(&task.goal, goal_width);

    Line::from(vec![
        Span::styled(
            format!(" {} ", status_char),
            Style::default().fg(status_color),
        ),
        Span::styled(
            format!("{} ", id_short),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(goal),
        Span::styled(status_info, Style::default().fg(Color::DarkGray)),
    ])
}

fn draw_topic(frame: &mut Frame, area: Rect, app: &App) {
    let topic = format!(" {} ", app.scope());
    let bar = Paragraph::new(topic).style(Style::default().bg(Color::Blue).fg(Color::White));
    frame.render_widget(bar, area);
}

fn draw_messages(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    let visible_height = area.height as usize;

    let all_lines: Vec<Line> = app
        .messages()
        .iter()
        .flat_map(|msg| format_message_lines(msg, width))
        .collect();

    let total = all_lines.len();
    let offset = app.scroll_offset();

    let start = total.saturating_sub(visible_height + offset);
    let end = total.saturating_sub(offset);

    let visible: Vec<Line> = all_lines
        .into_iter()
        .skip(start)
        .take(end - start)
        .collect();

    let para = Paragraph::new(visible).style(Style::default().bg(BG_MAIN));
    frame.render_widget(para, area);
}

fn draw_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let now: DateTime<Local> = Local::now();
    let clock = now.format("%H:%M").to_string();

    let scope = app.scope().to_string();
    let msg_count = app.messages().len();

    let activity = format_activity(app);

    let left = format!(" [{}] {} ", msg_count, scope);
    let right = format!(" {} ", clock);

    let activity_len = if activity.is_empty() {
        0
    } else {
        activity.len() + 2
    };
    let used = left.len() + right.len() + activity_len;
    let padding = (area.width as usize).saturating_sub(used);
    let middle = " ".repeat(padding);

    let mut spans = vec![
        Span::styled(left, Style::default().fg(Color::White)),
        Span::raw(middle),
    ];

    if !activity.is_empty() {
        spans.push(Span::styled(
            format!("[{}] ", activity),
            Style::default().fg(Color::Magenta),
        ));
    }

    spans.push(Span::styled(right, Style::default().fg(Color::White)));

    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Blue));
    frame.render_widget(bar, area);
}

fn format_activity(app: &App) -> String {
    let activity = app.activity();
    if activity.is_empty() {
        return String::new();
    }

    let mut items: Vec<_> = activity.iter().collect();
    items.sort_by_key(|(k, _)| k.as_str());

    items
        .iter()
        .map(|(scope, count)| format!("{}:{}", scope, count))
        .collect::<Vec<_>>()
        .join(" ")
}

fn draw_input(frame: &mut Frame, area: Rect, app: &App) {
    let input = Paragraph::new(format!("[{}] {}", app.scope(), app.input()));
    frame.render_widget(input, area);
}

fn format_message_lines(msg: &crate::bus::Message, width: usize) -> Vec<Line<'static>> {
    let timestamp = format_timestamp(msg.timestamp);
    let sender = format_sender(&msg.sender, &msg.origin);
    let content = format_content(&msg.op, &msg.data);

    if content.is_empty() {
        return vec![];
    }

    let content_width = width.saturating_sub(PREFIX_WIDTH);
    if content_width == 0 {
        return vec![];
    }

    let content_lines = split_and_wrap(&content, content_width);
    let mut result = Vec::with_capacity(content_lines.len());

    for (i, line_content) in content_lines.into_iter().enumerate() {
        let spans = if i == 0 {
            let mut s = vec![
                Span::styled(timestamp.clone(), Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
                sender.clone(),
                Span::raw(" "),
            ];
            s.extend(parse_markdown(&line_content));
            s
        } else {
            let indent = " ".repeat(PREFIX_WIDTH);
            let mut s = vec![Span::raw(indent)];
            s.extend(parse_markdown(&line_content));
            s
        };
        result.push(Line::from(spans));
    }

    result
}

fn split_and_wrap(text: &str, max_width: usize) -> Vec<String> {
    let mut result = Vec::new();

    for line in text.lines() {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }

        let mut current = String::new();
        for word in line.split_whitespace() {
            if current.is_empty() {
                if word.len() > max_width {
                    for chunk in word.as_bytes().chunks(max_width) {
                        result.push(String::from_utf8_lossy(chunk).into_owned());
                    }
                } else {
                    current = word.to_string();
                }
            } else if current.len() + 1 + word.len() <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                result.push(current);
                if word.len() > max_width {
                    for chunk in word.as_bytes().chunks(max_width) {
                        result.push(String::from_utf8_lossy(chunk).into_owned());
                    }
                    current = String::new();
                } else {
                    current = word.to_string();
                }
            }
        }
        if !current.is_empty() {
            result.push(current);
        }
    }

    if result.is_empty() {
        result.push(String::new());
    }

    result
}

fn parse_markdown(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut current = String::new();
    let mut in_bold = false;
    let mut in_italic = false;
    let mut in_code = false;

    while let Some((i, c)) = chars.next() {
        match c {
            '`' if !in_bold && !in_italic => {
                if !current.is_empty() {
                    spans.push(make_span(&current, in_bold, in_italic, in_code));
                    current.clear();
                }
                in_code = !in_code;
            }
            '*' if !in_code => {
                if chars.peek().map(|(_, c)| *c) == Some('*') {
                    chars.next();
                    if !current.is_empty() {
                        spans.push(make_span(&current, in_bold, in_italic, in_code));
                        current.clear();
                    }
                    in_bold = !in_bold;
                } else {
                    if !current.is_empty() {
                        spans.push(make_span(&current, in_bold, in_italic, in_code));
                        current.clear();
                    }
                    in_italic = !in_italic;
                }
            }
            '_' if !in_code && !in_bold => {
                let next_is_underscore = chars.peek().map(|(_, c)| *c) == Some('_');
                let prev_is_space = i == 0 || text.as_bytes().get(i - 1) == Some(&b' ');
                let next_is_space = chars.peek().map(|(_, c)| *c == ' ').unwrap_or(true);

                if next_is_underscore {
                    chars.next();
                    if !current.is_empty() {
                        spans.push(make_span(&current, in_bold, in_italic, in_code));
                        current.clear();
                    }
                    in_bold = !in_bold;
                } else if prev_is_space || next_is_space || in_italic {
                    if !current.is_empty() {
                        spans.push(make_span(&current, in_bold, in_italic, in_code));
                        current.clear();
                    }
                    in_italic = !in_italic;
                } else {
                    current.push(c);
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        spans.push(make_span(&current, in_bold, in_italic, in_code));
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }

    spans
}

fn make_span(text: &str, bold: bool, italic: bool, code: bool) -> Span<'static> {
    let mut style = Style::default();

    if code {
        style = style.fg(Color::Yellow);
    }
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if italic {
        style = style.add_modifier(Modifier::ITALIC);
    }

    Span::styled(text.to_string(), style)
}

fn format_timestamp(ts: SystemTime) -> String {
    let dt: DateTime<Local> = ts.into();
    dt.format("%H:%M").to_string()
}

fn format_sender(sender: &str, origin: &Origin) -> Span<'static> {
    let color = match origin {
        Origin::Human => Color::Green,
        Origin::Head => Color::Cyan,
        Origin::Hand => Color::Yellow,
        Origin::System => Color::DarkGray,
    };

    let truncated = if sender.len() > NICK_WIDTH {
        &sender[..NICK_WIDTH]
    } else {
        sender
    };

    let padded = format!("{:>width$}", truncated, width = NICK_WIDTH);
    Span::styled(padded, Style::default().fg(color))
}

fn format_content(op: &MessageOp, data: &MessageData) -> String {
    match (op, data) {
        (MessageOp::Chat, MessageData::Text(t)) => t.clone(),
        (MessageOp::Task, MessageData::Task(task)) => format_task(task),
        (MessageOp::Ping, _) => String::new(),
        (op, data) => format!("{:?}: {:?}", op, data),
    }
}

fn format_task(task: &crate::bus::TaskMsg) -> String {
    use crate::bus::TaskMsg;
    match task {
        TaskMsg::Request { task_id, goal, .. } => {
            format!("task {} requested: {}", short_id(task_id), goal)
        }
        TaskMsg::Assigned {
            task_id, hand_id, ..
        } => format!("task {} -> {}", short_id(task_id), hand_id),
        TaskMsg::Progress { task_id, note, .. } => {
            format!("task {} progress: {}", short_id(task_id), note)
        }
        TaskMsg::Echo {
            task_id,
            tool,
            content,
            ..
        } => {
            format!(
                "task {} [{}]: {}",
                short_id(task_id),
                tool,
                truncate(content, 60)
            )
        }
        TaskMsg::Result {
            task_id,
            ok,
            summary,
            ..
        } => {
            let status = if *ok { "OK" } else { "FAIL" };
            format!(
                "task {} {}: {}",
                short_id(task_id),
                status,
                truncate(summary, 60)
            )
        }
    }
}

fn short_id(id: &str) -> &str {
    if id.len() > 8 {
        &id[..8]
    } else {
        id
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
