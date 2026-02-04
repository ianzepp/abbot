use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::widgets::{draw_header, draw_statusline, draw_top_nav, draw_view_picker};
use crate::App;

#[derive(Clone)]
pub struct ExplorerNode {
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
    pub content: Option<String>,
}

pub fn draw_explorer(f: &mut Frame, app: &App) {
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
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(h_chunks[1]);

    draw_top_nav(f, &app.theme, chunks[1], app.view, app.paused, app.queued_count, app.tick_count, app.connected, app.dark_mode);
    draw_explorer_header(f, app, chunks[3]);

    let panel_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(2, 3),
        ])
        .split(chunks[5]);

    draw_file_tree(f, app, panel_chunks[0]);
    draw_file_preview(f, app, panel_chunks[1]);

    draw_explorer_status(f, app, chunks[6]);

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }
}

fn draw_explorer_header(f: &mut Frame, app: &App, area: Rect) {
    draw_header(f, &app.theme, area, " Workspace Explorer", app.theme.border_yellow);
}

fn draw_file_tree(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let header = Paragraph::new(" Files")
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    let content_area = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

    let visible: Vec<(usize, &ExplorerNode)> = build_visible_tree(&app.explorer_tree);

    let rows: Vec<Row> = visible
        .iter()
        .enumerate()
        .map(|(i, (_, node))| {
            let is_selected = i == app.explorer_selected;
            let indent = "  ".repeat(node.depth);

            let icon = if node.is_dir {
                if node.expanded { "▼ " } else { "▶ " }
            } else {
                "  "
            };

            let marker = if is_selected { "●" } else { " " };
            let style = if is_selected {
                Style::default().fg(theme.text_primary)
            } else if node.is_dir {
                Style::default().fg(theme.border_cyan)
            } else {
                Style::default().fg(theme.text_dim)
            };

            Row::new(vec![
                Span::styled(marker, Style::default().fg(theme.border_yellow)),
                Span::styled(format!("{}{}{}", indent, icon, node.name), style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(10),
        ],
    );
    f.render_widget(table, content_area);
}

fn draw_file_preview(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let header_area = Rect::new(area.x, area.y, area.width, 1);

    let visible = build_visible_tree(&app.explorer_tree);
    let selected_node = visible.get(app.explorer_selected).map(|(_, n)| *n);

    let title = selected_node
        .map(|n| format!(" {}", n.name))
        .unwrap_or_else(|| " Preview".into());

    let header = Paragraph::new(title)
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    let content_area = Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), area.height.saturating_sub(3));

    let content = selected_node
        .and_then(|n| n.content.as_ref())
        .map(|c| c.as_str())
        .unwrap_or_else(|| {
            if selected_node.map(|n| n.is_dir).unwrap_or(false) {
                "(directory)"
            } else {
                "(no preview available)"
            }
        });

    let preview = Paragraph::new(content)
        .style(Style::default().fg(theme.text_dim))
        .wrap(Wrap { trim: false });
    f.render_widget(preview, content_area);
}

pub fn build_visible_tree(tree: &[ExplorerNode]) -> Vec<(usize, &ExplorerNode)> {
    let mut visible = Vec::new();
    let mut skip_until_depth: Option<usize> = None;

    for (i, node) in tree.iter().enumerate() {
        if let Some(skip_depth) = skip_until_depth {
            if node.depth > skip_depth {
                continue;
            } else {
                skip_until_depth = None;
            }
        }

        visible.push((i, node));

        if node.is_dir && !node.expanded {
            skip_until_depth = Some(node.depth);
        }
    }

    visible
}

fn draw_explorer_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let time = chrono::Local::now().format("%H:%M");

    let visible = build_visible_tree(&app.explorer_tree);
    let selected_name = visible
        .get(app.explorer_selected)
        .map(|(_, n)| n.name.as_str())
        .unwrap_or("-");

    let left = Line::from(vec![
        Span::styled(format!("[{}]", time), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(format!(" [{}]", selected_name), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
    ]);

    draw_statusline(f, theme, area, left, "[^T] [^C]", theme.border_yellow);
}
