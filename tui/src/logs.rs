use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};
use tui_input::Input;

use crate::widgets::{draw_header, draw_statusline, draw_top_nav, draw_view_picker};
use crate::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogsFocus {
    List,
    Search,
}

impl Default for LogsFocus {
    fn default() -> Self {
        Self::List
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchField {
    Query,
    Name,
    Ops,
    Actors,
}

impl SearchField {
    pub fn label(&self) -> &'static str {
        match self {
            SearchField::Query => "Search text",
            SearchField::Name => "Syscall name",
            SearchField::Ops => "Op type",
            SearchField::Actors => "Actor",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            SearchField::Query => SearchField::Name,
            SearchField::Name => SearchField::Ops,
            SearchField::Ops => SearchField::Actors,
            SearchField::Actors => SearchField::Query,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            SearchField::Query => SearchField::Actors,
            SearchField::Name => SearchField::Query,
            SearchField::Ops => SearchField::Name,
            SearchField::Actors => SearchField::Ops,
        }
    }
}

#[derive(Debug, Default)]
pub struct LogsState {
    pub focus: LogsFocus,
    pub search_field: SearchField,
    pub query_input: Input,
    pub name_input: Input,
    pub ops_input: Input,
    pub actors_input: Input,
    pub selected: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub show_detail: bool,
}

impl LogsState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_input(&self) -> &Input {
        match self.search_field {
            SearchField::Query => &self.query_input,
            SearchField::Name => &self.name_input,
            SearchField::Ops => &self.ops_input,
            SearchField::Actors => &self.actors_input,
        }
    }

    pub fn current_input_mut(&mut self) -> &mut Input {
        match self.search_field {
            SearchField::Query => &mut self.query_input,
            SearchField::Name => &mut self.name_input,
            SearchField::Ops => &mut self.ops_input,
            SearchField::Actors => &mut self.actors_input,
        }
    }

    pub fn build_query_string(&self) -> String {
        let mut params = Vec::new();

        let query = self.query_input.value().trim();
        if !query.is_empty() {
            params.push(format!("query={}", simple_encode(query)));
        }

        let name = self.name_input.value().trim();
        if !name.is_empty() {
            params.push(format!("name={}", simple_encode(name)));
        }

        let ops = self.ops_input.value().trim();
        if !ops.is_empty() {
            params.push(format!("ops={}", simple_encode(ops)));
        }

        let actors = self.actors_input.value().trim();
        if !actors.is_empty() {
            params.push(format!("actors={}", simple_encode(actors)));
        }

        params.push("limit=200".to_string());
        params.push("order=desc".to_string());

        params.join("&")
    }
}

fn simple_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '%' => "%25".to_string(),
            '+' => "%2B".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

impl Default for SearchField {
    fn default() -> Self {
        Self::Query
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct LogEntry {
    pub seq: u64,
    pub ts_ms: i64,
    pub op: String,
    pub name: Option<String>,
    pub actor: Option<String>,
    pub frame_id: String,
    pub scope: Option<String>,
    pub frame: serde_json::Value,
}

pub fn draw_logs(f: &mut Frame, app: &App) {
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
            Constraint::Length(1),  // top margin
            Constraint::Length(1),  // top nav
            Constraint::Length(1),  // margin
            Constraint::Length(3),  // header
            Constraint::Length(1),  // margin
            Constraint::Min(10),    // content
            Constraint::Length(1),  // status
        ])
        .split(h_chunks[1]);

    draw_top_nav(f, &app.theme, chunks[1], app.view, app.paused, app.queued_count, app.tick_count, app.connected);
    draw_logs_header(f, app, chunks[3]);

    let panel_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Percentage(25),
        ])
        .split(chunks[5]);

    draw_logs_list(f, app, panel_chunks[0]);
    draw_search_panel(f, app, panel_chunks[1]);

    draw_logs_status(f, app, chunks[6]);

    if app.logs_state.show_detail {
        draw_log_detail(f, app);
    }

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }
}

fn draw_logs_header(f: &mut Frame, app: &App, area: Rect) {
    draw_header(f, &app.theme, area, " Server Logs", app.theme.border_yellow);
}

fn draw_logs_list(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let state = &app.logs_state;
    let focus_here = state.focus == LogsFocus::List;

    if state.loading {
        let loading = Paragraph::new("  Loading...")
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(loading, area);
        return;
    }

    if let Some(ref err) = state.error {
        let error = Paragraph::new(format!("  Error: {}", err))
            .style(Style::default().fg(theme.error_fg));
        f.render_widget(error, area);
        return;
    }

    if app.logs.is_empty() {
        let placeholder = Paragraph::new("  Waiting for logs...")
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(placeholder, area);
        return;
    }

    let rows: Vec<Row> = app.logs
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = i == state.selected;
            let is_focused = is_selected && focus_here;
            let marker = if is_focused { "\u{25cf}" } else { " " };

            let name = entry.name.as_deref().unwrap_or("-");
            let actor = entry.actor.as_deref().unwrap_or("-");

            let style = if is_selected {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.text_dim)
            };

            let mut row = Row::new(vec![
                Span::styled(marker, Style::default().fg(theme.border_cyan)),
                Span::styled(format!(" {:>6}", entry.seq), Style::default().fg(theme.text_dim)),
                Span::styled(format!(" {:6}", entry.op), Style::default().fg(theme.border_yellow)),
                Span::styled(format!(" {:20}", truncate(name, 20)), style),
                Span::styled(format!(" {}", truncate(actor, 30)), Style::default().fg(theme.text_secondary)),
            ]);

            if is_focused {
                row = row.style(Style::default().bg(theme.panel_header_bg));
            }
            row
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(21),
            Constraint::Min(10),
        ],
    )
    .column_spacing(0);

    f.render_widget(table, area);
}

fn draw_search_panel(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let state = &app.logs_state;
    let focus_here = state.focus == LogsFocus::Search;

    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(theme.text_dim))
        .style(Style::default().bg(theme.panel_bg));
    f.render_widget(block, area);

    let inner = Rect::new(area.x + 2, area.y + 1, area.width.saturating_sub(3), area.height.saturating_sub(2));

    let fields = [
        (SearchField::Query, &state.query_input),
        (SearchField::Name, &state.name_input),
        (SearchField::Ops, &state.ops_input),
        (SearchField::Actors, &state.actors_input),
    ];

    let mut y = inner.y;
    for (field, input) in fields.iter() {
        if y >= inner.y + inner.height {
            break;
        }

        let is_selected = focus_here && state.search_field == *field;
        let label_style = if is_selected {
            Style::default().fg(theme.text_primary)
        } else {
            Style::default().fg(theme.text_dim)
        };

        let label = Paragraph::new(field.label()).style(label_style);
        f.render_widget(label, Rect::new(inner.x, y, inner.width, 1));
        y += 1;

        if y >= inner.y + inner.height {
            break;
        }

        let input_style = if is_selected {
            Style::default().fg(theme.text_primary).bg(theme.panel_header_bg)
        } else {
            Style::default().fg(theme.text_secondary)
        };

        let display_value = if input.value().is_empty() && !is_selected {
            "(any)".to_string()
        } else {
            input.value().to_string()
        };

        let input_widget = Paragraph::new(display_value).style(input_style);
        let input_area = Rect::new(inner.x, y, inner.width, 1);
        f.render_widget(input_widget, input_area);

        if is_selected {
            let cursor_x = inner.x + input.visual_cursor() as u16;
            f.set_cursor_position((cursor_x.min(inner.x + inner.width - 1), y));
        }

        y += 2;
    }
}

fn draw_logs_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let state = &app.logs_state;

    let focus_text = match state.focus {
        LogsFocus::List => "LIST",
        LogsFocus::Search => "SEARCH",
    };

    let left = Line::from(vec![
        Span::styled(
            format!("[lines:{}]", app.logs.len()),
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
        Span::styled(
            format!(" [{}]", focus_text),
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
    ]);

    draw_statusline(f, theme, area, left, "[Tab] [Enter] [^R] [^T]", theme.border_yellow);
}

fn draw_log_detail(f: &mut Frame, app: &App) {
    let theme = &app.theme;
    let state = &app.logs_state;

    let Some(entry) = app.logs.get(state.selected) else {
        return;
    };

    let area = f.area();
    let dialog_width = (area.width as f32 * 0.8) as u16;
    let dialog_height = (area.height as f32 * 0.8) as u16;
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    f.render_widget(Clear, dialog_area);

    let json_str = serde_json::to_string_pretty(&entry.frame).unwrap_or_else(|_| "{}".to_string());

    let block = Block::default()
        .title(format!(" Log #{} ", entry.seq))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_yellow))
        .style(Style::default().bg(theme.header_bg));

    let inner = block.inner(dialog_area);
    f.render_widget(block, dialog_area);

    let content = Paragraph::new(json_str)
        .style(Style::default().fg(theme.text_primary))
        .wrap(Wrap { trim: false });
    f.render_widget(content, inner);
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
