use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table},
    Frame,
};

use crate::widgets::{draw_statusline, draw_top_nav, draw_view_picker};
use crate::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFocus {
    Sections,
    Fields,
    Dialog,
}

impl Default for ConfigFocus {
    fn default() -> Self {
        Self::Sections
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    Password,
    Number,
    Toggle,
    Select,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Text(String),
    Number(f64),
    Bool(bool),
    Selected(usize, Vec<String>),
    None,
}

impl FieldValue {
    pub fn display(&self) -> String {
        match self {
            FieldValue::Text(s) => {
                if s.is_empty() {
                    "(not set)".to_string()
                } else {
                    s.clone()
                }
            }
            FieldValue::Number(n) => n.to_string(),
            FieldValue::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            FieldValue::Selected(idx, options) => {
                options.get(*idx).cloned().unwrap_or_else(|| "(not set)".to_string())
            }
            FieldValue::None => "(not set)".to_string(),
        }
    }

    pub fn as_text(&self) -> String {
        match self {
            FieldValue::Text(s) => s.clone(),
            FieldValue::Number(n) => n.to_string(),
            FieldValue::Bool(b) => b.to_string(),
            FieldValue::Selected(idx, opts) => opts.get(*idx).cloned().unwrap_or_default(),
            FieldValue::None => String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigField {
    pub key: String,
    pub field_type: FieldType,
    pub value: FieldValue,
    pub original: FieldValue,
}

impl ConfigField {
    pub fn is_dirty(&self) -> bool {
        self.value != self.original
    }

    pub fn text(key: impl Into<String>, value: Option<String>) -> Self {
        let val = FieldValue::Text(value.unwrap_or_default());
        Self {
            key: key.into(),
            field_type: FieldType::Text,
            value: val.clone(),
            original: val,
        }
    }

    pub fn password(key: impl Into<String>, value: Option<String>) -> Self {
        let val = FieldValue::Text(value.unwrap_or_default());
        Self {
            key: key.into(),
            field_type: FieldType::Password,
            value: val.clone(),
            original: val,
        }
    }

    pub fn number(key: impl Into<String>, value: Option<f64>) -> Self {
        let val = value.map(FieldValue::Number).unwrap_or(FieldValue::None);
        Self {
            key: key.into(),
            field_type: FieldType::Number,
            value: val.clone(),
            original: val,
        }
    }

    pub fn toggle(key: impl Into<String>, value: Option<bool>) -> Self {
        let val = value.map(FieldValue::Bool).unwrap_or(FieldValue::None);
        Self {
            key: key.into(),
            field_type: FieldType::Toggle,
            value: val.clone(),
            original: val,
        }
    }

    pub fn select(key: impl Into<String>, options: Vec<String>, selected: Option<usize>) -> Self {
        let val = FieldValue::Selected(selected.unwrap_or(0), options);
        Self {
            key: key.into(),
            field_type: FieldType::Select,
            value: val.clone(),
            original: val,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigSection {
    pub name: String,
    pub fields: Vec<ConfigField>,
}

impl ConfigSection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            fields: Vec::new(),
        }
    }

    pub fn with_field(mut self, field: ConfigField) -> Self {
        self.fields.push(field);
        self
    }

    pub fn is_dirty(&self) -> bool {
        self.fields.iter().any(|f| f.is_dirty())
    }
}

#[derive(Debug, Clone)]
pub struct ConfigDialog {
    pub field_key: String,
    pub field_type: FieldType,
    pub input: String,
    pub cursor: usize,
    pub selected: usize,
    pub options: Vec<String>,
}

impl ConfigDialog {
    pub fn for_field(field: &ConfigField) -> Self {
        let (input, selected, options) = match &field.value {
            FieldValue::Text(s) => (s.clone(), 0, Vec::new()),
            FieldValue::Number(n) => (n.to_string(), 0, Vec::new()),
            FieldValue::Bool(b) => (String::new(), if *b { 0 } else { 1 }, vec!["Yes".into(), "No".into()]),
            FieldValue::Selected(idx, opts) => (String::new(), *idx, opts.clone()),
            FieldValue::None => (String::new(), 0, Vec::new()),
        };
        let cursor = input.len();
        Self {
            field_key: field.key.clone(),
            field_type: field.field_type,
            input,
            cursor,
            selected,
            options,
        }
    }

    pub fn to_value(&self) -> FieldValue {
        match self.field_type {
            FieldType::Text | FieldType::Password => FieldValue::Text(self.input.clone()),
            FieldType::Number => {
                self.input.parse::<f64>().ok()
                    .map(FieldValue::Number)
                    .unwrap_or(FieldValue::None)
            }
            FieldType::Toggle => FieldValue::Bool(self.selected == 0),
            FieldType::Select => FieldValue::Selected(self.selected, self.options.clone()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConfigEditorState {
    pub focus: ConfigFocus,
    pub sections: Vec<ConfigSection>,
    pub selected_section: usize,
    pub selected_field: usize,
    pub dialog: Option<ConfigDialog>,
    pub dirty: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub save_confirm: bool,
}

impl ConfigEditorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_section(&self) -> Option<&ConfigSection> {
        self.sections.get(self.selected_section)
    }

    pub fn current_section_mut(&mut self) -> Option<&mut ConfigSection> {
        self.sections.get_mut(self.selected_section)
    }

    pub fn current_field(&self) -> Option<&ConfigField> {
        self.current_section()
            .and_then(|s| s.fields.get(self.selected_field))
    }

    pub fn current_field_mut(&mut self) -> Option<&mut ConfigField> {
        let idx = self.selected_field;
        self.current_section_mut()
            .and_then(|s| s.fields.get_mut(idx))
    }

    pub fn is_any_dirty(&self) -> bool {
        self.sections.iter().any(|s| s.is_dirty())
    }

    pub fn load_from_json(&mut self, json: serde_json::Value) {
        self.sections.clear();
        self.loading = false;
        self.error = None;

        if let Some(obj) = json.as_object() {
            if let Some(workspace) = obj.get("workspace") {
                self.sections.push(
                    ConfigSection::new("workspace")
                        .with_field(ConfigField::text("path", workspace.as_str().map(|s| s.to_string())))
                );
            }

            if let Some(server) = obj.get("server").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("server");
                section.fields.push(ConfigField::text("addr", server.get("addr").and_then(|v| v.as_str()).map(|s| s.to_string())));
                section.fields.push(ConfigField::text("log_format", server.get("log_format").and_then(|v| v.as_str()).map(|s| s.to_string())));
                section.fields.push(ConfigField::toggle("reset_on_single_user_message", server.get("reset_on_single_user_message").and_then(|v| v.as_bool())));
                section.fields.push(ConfigField::text("proxy_base_url", server.get("proxy_base_url").and_then(|v| v.as_str()).map(|s| s.to_string())));
                section.fields.push(ConfigField::text("web_dist", server.get("web_dist").and_then(|v| v.as_str()).map(|s| s.to_string())));
                self.sections.push(section);
            }

            if let Some(head) = obj.get("head").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("head");
                section.fields.push(ConfigField::text("model", head.get("model").and_then(|v| v.as_str()).map(|s| s.to_string())));
                section.fields.push(ConfigField::number("temperature", head.get("temperature").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("max_tokens", head.get("max_tokens").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("heartbeat_tick", head.get("heartbeat_tick").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("debounce_ms", head.get("debounce_ms").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("pool", head.get("pool").and_then(|v| v.as_f64())));
                self.sections.push(section);
            }

            if let Some(hand) = obj.get("hand").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("hand");
                section.fields.push(ConfigField::text("model", hand.get("model").and_then(|v| v.as_str()).map(|s| s.to_string())));
                section.fields.push(ConfigField::number("temperature", hand.get("temperature").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("max_tokens", hand.get("max_tokens").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("max_iters", hand.get("max_iters").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("pool", hand.get("pool").and_then(|v| v.as_f64())));
                self.sections.push(section);
            }

            if let Some(mind) = obj.get("mind").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("mind");
                section.fields.push(ConfigField::text("model", mind.get("model").and_then(|v| v.as_str()).map(|s| s.to_string())));
                section.fields.push(ConfigField::number("temperature", mind.get("temperature").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("max_tokens", mind.get("max_tokens").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("tick_interval", mind.get("tick_interval").and_then(|v| v.as_f64())));
                self.sections.push(section);
            }

            if let Some(pool) = obj.get("pool").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("pool");
                section.fields.push(ConfigField::number("size", pool.get("size").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("timeout_secs", pool.get("timeout_secs").and_then(|v| v.as_f64())));
                self.sections.push(section);
            }

            if let Some(harness) = obj.get("harness").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("harness");
                section.fields.push(ConfigField::text("model", harness.get("model").and_then(|v| v.as_str()).map(|s| s.to_string())));
                section.fields.push(ConfigField::number("slow_idle", harness.get("slow_idle").and_then(|v| v.as_f64())));
                section.fields.push(ConfigField::number("deep_idle", harness.get("deep_idle").and_then(|v| v.as_f64())));
                self.sections.push(section);
            }

            if let Some(prompt_cache) = obj.get("prompt_cache").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("prompt_cache");
                section.fields.push(ConfigField::toggle("enabled", prompt_cache.get("enabled").and_then(|v| v.as_bool())));
                section.fields.push(ConfigField::text("model", prompt_cache.get("model").and_then(|v| v.as_str()).map(|s| s.to_string())));
                self.sections.push(section);
            }
        }

        self.selected_section = 0;
        self.selected_field = 0;
        self.focus = ConfigFocus::Sections;
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();

        for section in &self.sections {
            match section.name.as_str() {
                "workspace" => {
                    if let Some(field) = section.fields.first() {
                        if let FieldValue::Text(s) = &field.value {
                            if !s.is_empty() {
                                obj.insert("workspace".into(), serde_json::Value::String(s.clone()));
                            }
                        }
                    }
                }
                name => {
                    let mut section_obj = serde_json::Map::new();
                    for field in &section.fields {
                        let json_val = match &field.value {
                            FieldValue::Text(s) if !s.is_empty() => Some(serde_json::Value::String(s.clone())),
                            FieldValue::Number(n) => Some(serde_json::json!(*n)),
                            FieldValue::Bool(b) => Some(serde_json::Value::Bool(*b)),
                            FieldValue::Selected(idx, opts) => opts.get(*idx).map(|s| serde_json::Value::String(s.clone())),
                            _ => None,
                        };
                        if let Some(val) = json_val {
                            section_obj.insert(field.key.clone(), val);
                        }
                    }
                    if !section_obj.is_empty() {
                        obj.insert(name.to_string(), serde_json::Value::Object(section_obj));
                    }
                }
            }
        }

        serde_json::Value::Object(obj)
    }
}

pub fn draw_config(f: &mut Frame, app: &App) {
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
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(h_chunks[1]);

    draw_top_nav(f, &app.theme, chunks[1], app.view, app.paused, app.queued_count, app.tick_count, app.connected);

    let panel_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(26),
            Constraint::Min(10),
        ])
        .split(chunks[3]);

    draw_sections_panel(f, app, panel_chunks[0]);
    draw_fields_panel(f, app, panel_chunks[1]);
    draw_config_status(f, app, chunks[4]);

    if app.config_editor.dialog.is_some() {
        draw_dialog(f, app);
    }

    if app.config_editor.save_confirm {
        draw_save_confirm(f, app);
    }

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }
}

fn draw_sections_panel(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let editor = &app.config_editor;
    let focus_here = editor.focus == ConfigFocus::Sections;

    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let header = Paragraph::new(" Sections")
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    let content_area = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

    if editor.loading {
        let loading = Paragraph::new("  Loading...")
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(loading, content_area);
        return;
    }

    if let Some(ref err) = editor.error {
        let error = Paragraph::new(format!("  Error: {}", err))
            .style(Style::default().fg(theme.error_fg));
        f.render_widget(error, content_area);
        return;
    }

    let rows: Vec<Row> = editor
        .sections
        .iter()
        .enumerate()
        .map(|(i, section)| {
            let is_selected = i == editor.selected_section;
            let marker = if is_selected && focus_here {
                "\u{25cf}"
            } else {
                " "
            };
            let dirty_marker = if section.is_dirty() { "\u{25c6}" } else { " " };
            let style = if is_selected {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.text_dim)
            };

            Row::new(vec![
                Span::styled(format!(" {}", marker), Style::default().fg(theme.border_cyan)),
                Span::styled(dirty_marker, Style::default().fg(theme.border_yellow)),
                Span::styled(format!(" {}", section.name), style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(10),
        ],
    );
    f.render_widget(table, content_area);
}

fn draw_fields_panel(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let editor = &app.config_editor;
    let focus_here = editor.focus == ConfigFocus::Fields;

    let section_name = editor
        .current_section()
        .map(|s| s.name.as_str())
        .unwrap_or("-");

    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let header = Paragraph::new(format!(" [{}]", section_name))
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    let content_area = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

    let Some(section) = editor.current_section() else {
        return;
    };

    let rows: Vec<Row> = section
        .fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let is_selected = i == editor.selected_field;
            let marker = if is_selected && focus_here {
                "\u{25cf}"
            } else {
                " "
            };
            let dirty_marker = if field.is_dirty() { "\u{25c6}" } else { " " };
            let key_style = if is_selected {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.text_dim)
            };
            let value_style = Style::default().fg(theme.text_secondary);

            let display_value = match field.field_type {
                FieldType::Password => {
                    if let FieldValue::Text(s) = &field.value {
                        if s.is_empty() {
                            "(not set)".to_string()
                        } else {
                            "*".repeat(s.len().min(16))
                        }
                    } else {
                        "(not set)".to_string()
                    }
                }
                _ => field.value.display(),
            };

            Row::new(vec![
                Span::styled(format!(" {}", marker), Style::default().fg(theme.border_cyan)),
                Span::styled(dirty_marker, Style::default().fg(theme.border_yellow)),
                Span::styled(format!(" {:20}", field.key), key_style),
                Span::styled(display_value, value_style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(22),
            Constraint::Min(10),
        ],
    );
    f.render_widget(table, content_area);
}

fn draw_dialog(f: &mut Frame, app: &App) {
    let theme = &app.theme;
    let Some(dialog) = &app.config_editor.dialog else {
        return;
    };

    let area = f.area();
    let dialog_width = 50u16.min(area.width.saturating_sub(4));
    let dialog_height = match dialog.field_type {
        FieldType::Toggle | FieldType::Select => (dialog.options.len() + 4) as u16,
        _ => 5,
    };

    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    f.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_cyan))
        .title(format!(" Edit: {} ", dialog.field_key))
        .style(Style::default().bg(theme.panel_bg));
    f.render_widget(block, dialog_area);

    let inner = Rect::new(
        dialog_area.x + 2,
        dialog_area.y + 1,
        dialog_area.width.saturating_sub(4),
        dialog_area.height.saturating_sub(2),
    );

    match dialog.field_type {
        FieldType::Text | FieldType::Number => {
            let input_line = format!("> {}", dialog.input);
            let input = Paragraph::new(input_line)
                .style(Style::default().fg(theme.text_primary));
            f.render_widget(input, inner);

            let cursor_x = inner.x + 2 + dialog.cursor as u16;
            f.set_cursor_position((cursor_x.min(inner.x + inner.width - 1), inner.y));
        }
        FieldType::Password => {
            let masked = "*".repeat(dialog.input.len());
            let input_line = format!("> {}", masked);
            let input = Paragraph::new(input_line)
                .style(Style::default().fg(theme.text_primary));
            f.render_widget(input, inner);

            let cursor_x = inner.x + 2 + dialog.cursor as u16;
            f.set_cursor_position((cursor_x.min(inner.x + inner.width - 1), inner.y));
        }
        FieldType::Toggle | FieldType::Select => {
            for (i, opt) in dialog.options.iter().enumerate() {
                let opt_area = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
                let marker = if i == dialog.selected { "\u{25cf}" } else { "\u{25cb}" };
                let style = if i == dialog.selected {
                    Style::default().fg(theme.border_cyan)
                } else {
                    Style::default().fg(theme.text_dim)
                };
                let line = Line::from(vec![
                    Span::styled(format!(" {} ", marker), style),
                    Span::styled(opt, style),
                ]);
                f.render_widget(Paragraph::new(line), opt_area);
            }
        }
    }

    let hint_area = Rect::new(
        dialog_area.x + 2,
        dialog_area.y + dialog_area.height - 2,
        dialog_area.width.saturating_sub(4),
        1,
    );
    let hint = Paragraph::new("[Enter] Save  [Esc] Cancel")
        .style(Style::default().fg(theme.text_dim));
    f.render_widget(hint, hint_area);
}

fn draw_save_confirm(f: &mut Frame, app: &App) {
    let theme = &app.theme;
    let area = f.area();

    let dialog_width = 30u16;
    let dialog_height = 5u16;
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    f.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_yellow))
        .title(" Save Changes? ")
        .style(Style::default().bg(theme.panel_bg));
    f.render_widget(block, dialog_area);

    let inner = Rect::new(
        dialog_area.x + 2,
        dialog_area.y + 2,
        dialog_area.width.saturating_sub(4),
        1,
    );
    let hint = Paragraph::new("[Y] Yes  [N/Esc] No")
        .style(Style::default().fg(theme.text_primary));
    f.render_widget(hint, inner);
}

fn draw_config_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let editor = &app.config_editor;

    let section_name = editor
        .current_section()
        .map(|s| s.name.as_str())
        .unwrap_or("-");

    let dirty_indicator = if editor.is_any_dirty() {
        " [unsaved changes]"
    } else {
        ""
    };

    let focus_indicator = match editor.focus {
        ConfigFocus::Sections => "Sections",
        ConfigFocus::Fields => "Fields",
        ConfigFocus::Dialog => "Editing",
    };

    let left = Line::from(vec![
        Span::styled(
            format!("[{}] ", section_name),
            Style::default().bg(theme.header_bg).fg(theme.text_primary),
        ),
        Span::styled(
            focus_indicator,
            Style::default().bg(theme.header_bg).fg(theme.text_dim),
        ),
        Span::styled(
            dirty_indicator,
            Style::default().bg(theme.header_bg).fg(theme.border_yellow),
        ),
    ]);

    draw_statusline(f, theme, area, left, "[^S Save] [^T] [^C]", theme.border_magenta);
}
