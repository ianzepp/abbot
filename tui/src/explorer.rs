//! Workspace explorer — file tree browser with split-panel preview.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Row, Table, Wrap},
};

use crate::App;
use crate::widgets::{
    draw_header_with_right, draw_statusline, draw_subheader, draw_top_nav, draw_view_picker,
    ellipsize_left,
};

/// A single entry in the flat workspace file tree.
///
/// The tree is stored as a flat `Vec` ordered by DFS traversal. `depth`
/// encodes nesting so `build_visible_tree` can skip collapsed subtrees
/// without a recursive data structure.
#[derive(Clone)]
pub struct ExplorerNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
    /// Whether children (dirs) or content (files) have been fetched from the daemon.
    pub loaded: bool,
    /// File content for preview; `None` until the user explicitly loads it.
    pub content: Option<String>,
}

pub fn draw_explorer(f: &mut Frame, app: &App) {
    // WHY: 1-char horizontal margins prevent content from touching terminal edges
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
    draw_explorer_header(f, app, chunks[3]);

    if app.explorer_tree.is_empty() && (app.explorer_loading || app.explorer_error.is_some()) {
        let msg = if app.explorer_error.is_some() {
            "  Waiting for connection...\n\n  Press Ctrl+R to retry."
        } else {
            "  Waiting for connection..."
        };
        let waiting = Paragraph::new(msg)
            .style(Style::default().fg(app.theme.text_dim))
            .wrap(Wrap { trim: false });
        f.render_widget(waiting, chunks[5]);
    } else {
        // WHY: 1/3 tree + 2/3 preview mirrors common IDE sidebar proportions
        let panel_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)])
            .split(chunks[5]);

        draw_file_tree(f, app, panel_chunks[0]);
        draw_file_preview(f, app, panel_chunks[1]);
    }

    draw_explorer_status(f, app, chunks[6]);

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }
}

fn draw_explorer_header(f: &mut Frame, app: &App, area: Rect) {
    let right = app
        .explorer_workspace
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();

    let title_area_width = area.width.saturating_sub(2) as usize;
    // WHY: reserve enough room for " Workspace Explorer" so the path never overwrites it
    let reserve_left = 22usize;
    let max_right = title_area_width.saturating_sub(reserve_left);
    let right = if max_right == 0 {
        String::new()
    } else {
        ellipsize_left(&right, max_right)
    };

    draw_header_with_right(
        f,
        &app.theme,
        area,
        " Workspace Explorer",
        &right,
        Style::default().fg(app.theme.text_dim),
        app.theme.border_yellow,
    );
}

fn draw_file_tree(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let header_area = Rect::new(area.x, area.y, area.width, 2);
    draw_subheader(f, theme, header_area, " Files", theme.border_yellow);

    let content_area = Rect::new(
        area.x,
        area.y + 2,
        area.width,
        area.height.saturating_sub(2),
    );

    let visible: Vec<(usize, &ExplorerNode)> = build_visible_tree(&app.explorer_tree);

    if visible.is_empty() {
        let placeholder = Paragraph::new("  (empty)").style(Style::default().fg(theme.text_dim));
        f.render_widget(placeholder, content_area);
        return;
    }

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

    let table = Table::new(rows, [Constraint::Length(1), Constraint::Min(10)]);
    f.render_widget(table, content_area);
}

fn draw_file_preview(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let header_area = Rect::new(area.x, area.y, area.width, 2);

    let visible = build_visible_tree(&app.explorer_tree);
    let selected_node = visible.get(app.explorer_selected).map(|(_, n)| *n);

    let title = selected_node
        .map(|n| format!(" {}", n.name))
        .unwrap_or_else(|| " Preview".into());

    draw_subheader(f, theme, header_area, &title, theme.border_yellow);

    // WHY: inset by 1 col and start 1 row below subheader border for visual padding
    let content_area = Rect::new(
        area.x + 1,
        area.y + 3,
        area.width.saturating_sub(2),
        area.height.saturating_sub(4),
    );

    let content = selected_node
        .and_then(|n| n.content.as_ref())
        .map(|c| c.as_str())
        .unwrap_or_else(|| {
            if selected_node.map(|n| n.is_dir).unwrap_or(false) {
                "(directory)"
            } else if app.explorer_loading {
                "(loading...)"
            } else {
                "(press Enter to load)"
            }
        });

    let preview = Paragraph::new(content)
        .style(Style::default().fg(theme.text_dim))
        .wrap(Wrap { trim: false });
    f.render_widget(preview, content_area);
}

/// Walk the flat tree and return only the nodes that should be visible,
/// skipping children of collapsed directories. Each entry carries its
/// original index into `tree` so callers can map selection back to the
/// backing store.
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

    let visible = build_visible_tree(&app.explorer_tree);
    let selected_name = visible
        .get(app.explorer_selected)
        .map(|(_, n)| {
            if n.path.is_empty() {
                "."
            } else {
                n.path.as_str()
            }
        })
        .unwrap_or("-");

    let loading = if app.explorer_loading {
        " [loading]"
    } else {
        ""
    };

    let left = Line::from(vec![Span::styled(
        format!("[{}]{}", selected_name, loading),
        Style::default().bg(theme.header_bg).fg(theme.text_primary),
    )]);

    draw_statusline(
        f,
        theme,
        area,
        left,
        "[Enter] [^R] [^T]",
        theme.border_yellow,
    );
}
