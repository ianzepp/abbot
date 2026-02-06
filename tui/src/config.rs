use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap},
};

use crate::App;
use crate::widgets::{
    draw_header, draw_statusline, draw_subheader, draw_top_nav, draw_view_picker,
};

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
    Model,
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
            FieldValue::Number(n) => format_float(*n),
            FieldValue::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            FieldValue::Selected(idx, options) => options
                .get(*idx)
                .cloned()
                .unwrap_or_else(|| "(not set)".to_string()),
            FieldValue::None => "(not set)".to_string(),
        }
    }

    pub fn as_text(&self) -> String {
        match self {
            FieldValue::Text(s) => s.clone(),
            FieldValue::Number(n) => format_float(*n),
            FieldValue::Bool(b) => b.to_string(),
            FieldValue::Selected(idx, opts) => opts.get(*idx).cloned().unwrap_or_default(),
            FieldValue::None => String::new(),
        }
    }
}

fn format_float(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{:.0}", n)
    } else {
        let s = format!("{:.6}", n);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
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

    pub fn model(key: impl Into<String>, value: Option<String>) -> Self {
        let val = FieldValue::Text(value.unwrap_or_default());
        Self {
            key: key.into(),
            field_type: FieldType::Model,
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
    pub model_loading: bool,
    pub model_error: Option<String>,
    pub model_options: Vec<(String, String)>,
    pub model_current: Option<String>,
}

impl ConfigDialog {
    pub fn for_field(field: &ConfigField) -> Self {
        let (input, selected, options) = match &field.value {
            FieldValue::Text(s) => {
                if field.field_type == FieldType::Model {
                    (String::new(), 0, Vec::new())
                } else {
                    (s.clone(), 0, Vec::new())
                }
            }
            FieldValue::Number(n) => (n.to_string(), 0, Vec::new()),
            FieldValue::Bool(b) => (
                String::new(),
                if *b { 0 } else { 1 },
                vec!["Yes".into(), "No".into()],
            ),
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
            model_loading: field.field_type == FieldType::Model,
            model_error: None,
            model_options: Vec::new(),
            model_current: if field.field_type == FieldType::Model {
                Some(field.value.as_text())
            } else {
                None
            },
        }
    }

    pub(crate) fn set_model_options(&mut self, options: Vec<crate::ModelOption>) {
        if self.field_type != FieldType::Model {
            return;
        }
        self.model_loading = false;
        self.model_error = None;
        self.model_options = options.into_iter().map(|o| (o.id, o.display)).collect();
        if let Some(ref current) = self.model_current
            && let Some(i) = self.model_options.iter().position(|(id, _)| id == current)
        {
            self.selected = i;
            return;
        }
        self.selected = 0;
    }

    pub(crate) fn set_model_error(&mut self, error: String) {
        if self.field_type != FieldType::Model {
            return;
        }
        self.model_loading = false;
        self.model_error = Some(error);
    }

    pub(crate) fn model_filtered_indices(&self) -> Vec<usize> {
        let q = self.input.trim().to_ascii_lowercase();
        let mut idxs = Vec::new();
        for (i, (id, display)) in self.model_options.iter().enumerate() {
            if q.is_empty() {
                idxs.push(i);
                continue;
            }
            if id.to_ascii_lowercase().contains(&q) || display.to_ascii_lowercase().contains(&q) {
                idxs.push(i);
            }
        }
        idxs
    }

    pub fn to_value(&self) -> FieldValue {
        match self.field_type {
            FieldType::Text | FieldType::Password => FieldValue::Text(self.input.clone()),
            FieldType::Number => self
                .input
                .parse::<f64>()
                .ok()
                .map(FieldValue::Number)
                .unwrap_or(FieldValue::None),
            FieldType::Toggle => FieldValue::Bool(self.selected == 0),
            FieldType::Select => FieldValue::Selected(self.selected, self.options.clone()),
            FieldType::Model => {
                let idxs = self.model_filtered_indices();
                if let Some(i) = idxs.get(self.selected) {
                    FieldValue::Text(self.model_options[*i].0.clone())
                } else {
                    FieldValue::Text(self.input.trim().to_string())
                }
            }
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
    pub base_json: serde_json::Value,
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

    pub fn validate_for_save(&self) -> Result<(), String> {
        for section in &self.sections {
            for field in &section.fields {
                let FieldValue::Number(n) = field.value else {
                    continue;
                };

                let Some(kind) = numeric_kind(section.name.as_str(), field.key.as_str()) else {
                    continue;
                };

                if !n.is_finite() {
                    return Err(format!(
                        "{}.{} must be a finite number",
                        section.name, field.key
                    ));
                }

                match kind {
                    NumericKind::Float => {}
                    NumericKind::U32 => {
                        if n.fract() != 0.0 || n < 0.0 || n > u32::MAX as f64 {
                            return Err(format!(
                                "{}.{} must be an integer in [0, {}]",
                                section.name,
                                field.key,
                                u32::MAX
                            ));
                        }
                    }
                    NumericKind::U64 => {
                        if n.fract() != 0.0 || n < 0.0 || n > u64::MAX as f64 {
                            return Err(format!(
                                "{}.{} must be a non-negative integer",
                                section.name, field.key
                            ));
                        }
                    }
                    NumericKind::Usize => {
                        let max = usize::MAX as f64;
                        if n.fract() != 0.0 || n < 0.0 || n > max {
                            return Err(format!(
                                "{}.{} must be a non-negative integer",
                                section.name, field.key
                            ));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn load_from_json(&mut self, json: serde_json::Value) {
        self.base_json = json.clone();
        self.sections.clear();
        self.loading = false;
        self.error = None;

        if let Some(obj) = json.as_object() {
            if let Some(workspace) = obj.get("workspace") {
                self.sections.push(
                    ConfigSection::new("workspace").with_field(ConfigField::text(
                        "path",
                        workspace.as_str().map(|s| s.to_string()),
                    )),
                );
            }

            if let Some(server) = obj.get("server").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("server");
                section.fields.push(ConfigField::text(
                    "addr",
                    server
                        .get("addr")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ));
                section.fields.push(ConfigField::text(
                    "log_format",
                    server
                        .get("log_format")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ));
                section.fields.push(ConfigField::toggle(
                    "reset_on_single_user_message",
                    server
                        .get("reset_on_single_user_message")
                        .and_then(|v| v.as_bool()),
                ));
                section.fields.push(ConfigField::text(
                    "proxy_base_url",
                    server
                        .get("proxy_base_url")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ));
                section.fields.push(ConfigField::text(
                    "web_dist",
                    server
                        .get("web_dist")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ));
                self.sections.push(section);
            }

            if let Some(head) = obj.get("head").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("head");
                section.fields.push(ConfigField::model(
                    "model",
                    head.get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ));
                section.fields.push(ConfigField::number(
                    "temperature",
                    head.get("temperature").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "max_tokens",
                    head.get("max_tokens").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "heartbeat_tick",
                    head.get("heartbeat_tick").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "debounce_ms",
                    head.get("debounce_ms").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "pool",
                    head.get("pool").and_then(|v| v.as_f64()),
                ));
                self.sections.push(section);
            }

            if let Some(hand) = obj.get("hand").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("hand");
                section.fields.push(ConfigField::model(
                    "model",
                    hand.get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ));
                section.fields.push(ConfigField::number(
                    "temperature",
                    hand.get("temperature").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "max_tokens",
                    hand.get("max_tokens").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "max_iters",
                    hand.get("max_iters").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "pool",
                    hand.get("pool").and_then(|v| v.as_f64()),
                ));
                self.sections.push(section);
            }

            if let Some(mind) = obj.get("mind").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("mind");
                section.fields.push(ConfigField::model(
                    "model",
                    mind.get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ));
                section.fields.push(ConfigField::number(
                    "temperature",
                    mind.get("temperature").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "max_tokens",
                    mind.get("max_tokens").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "tick_interval",
                    mind.get("tick_interval").and_then(|v| v.as_f64()),
                ));
                self.sections.push(section);
            }

            if let Some(pool) = obj.get("pool").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("pool");
                section.fields.push(ConfigField::number(
                    "size",
                    pool.get("size").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "timeout_secs",
                    pool.get("timeout_secs").and_then(|v| v.as_f64()),
                ));
                self.sections.push(section);
            }

            if let Some(harness) = obj.get("harness").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("harness");
                section.fields.push(ConfigField::model(
                    "model",
                    harness
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ));
                section.fields.push(ConfigField::number(
                    "slow_idle",
                    harness.get("slow_idle").and_then(|v| v.as_f64()),
                ));
                section.fields.push(ConfigField::number(
                    "deep_idle",
                    harness.get("deep_idle").and_then(|v| v.as_f64()),
                ));
                self.sections.push(section);
            }

            if let Some(prompt_cache) = obj.get("prompt_cache").and_then(|v| v.as_object()) {
                let mut section = ConfigSection::new("prompt_cache");
                section.fields.push(ConfigField::toggle(
                    "enabled",
                    prompt_cache.get("enabled").and_then(|v| v.as_bool()),
                ));
                section.fields.push(ConfigField::model(
                    "model",
                    prompt_cache
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                ));
                self.sections.push(section);
            }
        }

        self.selected_section = 0;
        self.selected_field = 0;
        self.focus = ConfigFocus::Sections;
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = self
            .base_json
            .as_object()
            .cloned()
            .unwrap_or_else(serde_json::Map::new);

        for section in &self.sections {
            match section.name.as_str() {
                "workspace" => {
                    if let Some(field) = section.fields.first()
                        && let FieldValue::Text(s) = &field.value
                    {
                        if !s.is_empty() {
                            obj.insert("workspace".into(), serde_json::Value::String(s.clone()));
                        } else {
                            obj.remove("workspace");
                        }
                    }
                }
                name => {
                    let mut section_obj = obj
                        .get(name)
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_else(serde_json::Map::new);
                    for field in &section.fields {
                        let json_val = match &field.value {
                            FieldValue::Text(s) if !s.is_empty() => {
                                Some(serde_json::Value::String(s.clone()))
                            }
                            FieldValue::Number(n) => {
                                let n = *n;
                                if n.is_finite() && n.fract() == 0.0 {
                                    if n >= 0.0 && n <= u64::MAX as f64 {
                                        Some(serde_json::Value::Number(serde_json::Number::from(
                                            n as u64,
                                        )))
                                    } else if n >= i64::MIN as f64 && n <= i64::MAX as f64 {
                                        Some(serde_json::Value::Number(serde_json::Number::from(
                                            n as i64,
                                        )))
                                    } else {
                                        None
                                    }
                                } else {
                                    serde_json::Number::from_f64(n).map(serde_json::Value::Number)
                                }
                            }
                            FieldValue::Bool(b) => Some(serde_json::Value::Bool(*b)),
                            FieldValue::Selected(idx, opts) => {
                                opts.get(*idx).map(|s| serde_json::Value::String(s.clone()))
                            }
                            _ => None,
                        };
                        if let Some(val) = json_val {
                            section_obj.insert(field.key.clone(), val);
                        } else {
                            section_obj.remove(&field.key);
                        }
                    }
                    if !section_obj.is_empty() {
                        obj.insert(name.to_string(), serde_json::Value::Object(section_obj));
                    } else {
                        obj.remove(name);
                    }
                }
            }
        }

        serde_json::Value::Object(obj)
    }
}

#[derive(Debug, Clone, Copy)]
enum NumericKind {
    Float,
    U32,
    U64,
    Usize,
}

fn numeric_kind(section: &str, key: &str) -> Option<NumericKind> {
    match (section, key) {
        ("head", "temperature") => Some(NumericKind::Float),
        ("hand", "temperature") => Some(NumericKind::Float),
        ("mind", "temperature") => Some(NumericKind::Float),

        ("head", "max_tokens") => Some(NumericKind::U32),
        ("hand", "max_tokens") => Some(NumericKind::U32),
        ("mind", "max_tokens") => Some(NumericKind::U32),

        ("head", "heartbeat_tick") => Some(NumericKind::U64),
        ("head", "debounce_ms") => Some(NumericKind::U64),
        ("mind", "tick_interval") => Some(NumericKind::U64),
        ("pool", "timeout_secs") => Some(NumericKind::U64),
        ("harness", "slow_idle") => Some(NumericKind::U64),
        ("harness", "deep_idle") => Some(NumericKind::U64),

        ("head", "pool") => Some(NumericKind::Usize),
        ("hand", "max_iters") => Some(NumericKind::Usize),
        ("hand", "pool") => Some(NumericKind::Usize),
        ("pool", "size") => Some(NumericKind::Usize),

        _ => None,
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
            Constraint::Length(1), // top margin
            Constraint::Length(1), // top nav
            Constraint::Length(1), // margin
            Constraint::Length(3), // header
            Constraint::Length(1), // margin
            Constraint::Min(10),   // panels
            Constraint::Length(1), // status
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
    draw_config_header(f, app, chunks[3]);

    let editor = &app.config_editor;
    if editor.sections.is_empty() && (editor.loading || editor.error.is_some()) {
        let msg = if editor.error.is_some() {
            "  Waiting for connection...\n\n  Press Ctrl+R to retry."
        } else {
            "  Waiting for connection..."
        };
        let waiting = Paragraph::new(msg)
            .style(Style::default().fg(app.theme.text_dim))
            .wrap(Wrap { trim: false });
        f.render_widget(waiting, chunks[5]);
    } else {
        let panel_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(26), Constraint::Min(10)])
            .split(chunks[5]);

        let left_header = Rect::new(
            panel_chunks[0].x,
            panel_chunks[0].y,
            panel_chunks[0].width,
            2,
        );
        let left_content = Rect::new(
            panel_chunks[0].x,
            panel_chunks[0].y + 2,
            panel_chunks[0].width,
            panel_chunks[0].height.saturating_sub(2),
        );
        draw_subheader(
            f,
            &app.theme,
            left_header,
            " Sections",
            app.theme.border_magenta,
        );
        draw_sections_panel(f, app, left_content);

        let section_name = app
            .config_editor
            .current_section()
            .map(|s| s.name.as_str())
            .unwrap_or("Fields");
        let right_title = format!(" Fields: {}", section_name);

        let right_header = Rect::new(
            panel_chunks[1].x,
            panel_chunks[1].y,
            panel_chunks[1].width,
            2,
        );
        let right_content = Rect::new(
            panel_chunks[1].x,
            panel_chunks[1].y + 2,
            panel_chunks[1].width,
            panel_chunks[1].height.saturating_sub(2),
        );
        draw_subheader(
            f,
            &app.theme,
            right_header,
            &right_title,
            app.theme.border_magenta,
        );
        draw_fields_panel(f, app, right_content);
    }
    draw_config_status(f, app, chunks[6]);

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

fn draw_config_header(f: &mut Frame, app: &App, area: Rect) {
    draw_header(
        f,
        &app.theme,
        area,
        " Configuration Editor",
        app.theme.border_magenta,
    );
}

fn draw_sections_panel(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let editor = &app.config_editor;
    let focus_here = editor.focus == ConfigFocus::Sections;

    if editor.loading {
        let loading = Paragraph::new("  Loading...").style(Style::default().fg(theme.text_dim));
        f.render_widget(loading, area);
        return;
    }

    if let Some(ref err) = editor.error {
        let msg = format!(
            "Error saving/loading config:\n\n{}\n\nTip: Ctrl+R reloads from server.",
            err
        );
        let error = Paragraph::new(msg)
            .style(Style::default().fg(theme.error_fg))
            .wrap(Wrap { trim: false });
        f.render_widget(error, area);
        return;
    }

    let section_entered =
        editor.focus == ConfigFocus::Fields || editor.focus == ConfigFocus::Dialog;

    let rows: Vec<Row> = editor
        .sections
        .iter()
        .enumerate()
        .map(|(i, section)| {
            let is_selected = i == editor.selected_section;
            let is_focused = is_selected && focus_here;
            let marker = if is_focused { "\u{25cf}" } else { " " };
            let style = if is_selected {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.text_dim)
            };

            let mut cells = vec![
                Span::styled(marker, Style::default().fg(theme.border_cyan)),
                Span::styled(format!(" {}", section.name), style),
            ];
            if section.is_dirty() {
                cells.push(Span::styled(
                    " \u{25c6}",
                    Style::default().fg(theme.border_yellow),
                ));
            }

            let mut row = Row::new(cells);
            if is_selected && section_entered {
                row = row.style(Style::default().bg(theme.panel_header_bg));
            }
            row
        })
        .collect();

    let table = Table::new(rows, [Constraint::Length(1), Constraint::Min(10)]).column_spacing(0);
    f.render_widget(table, area);
}

fn draw_fields_panel(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let editor = &app.config_editor;
    let focus_here = editor.focus == ConfigFocus::Fields;
    let field_entered = editor.focus == ConfigFocus::Dialog;

    if let Some(ref err) = editor.error {
        let msg = format!(
            "Error saving/loading config:\n\n{}\n\nTip: Ctrl+R reloads from server.",
            err
        );
        let error = Paragraph::new(msg)
            .style(Style::default().fg(theme.error_fg))
            .wrap(Wrap { trim: false });
        f.render_widget(error, area);
        return;
    }

    let Some(section) = editor.current_section() else {
        return;
    };

    let rows: Vec<Row> = section
        .fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let is_selected = i == editor.selected_field;
            let is_focused = is_selected && focus_here;
            let marker = if is_focused { "\u{25cf}" } else { " " };
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

            let dirty_suffix = if field.is_dirty() { " \u{25c6}" } else { "" };

            let mut row = Row::new(vec![
                Span::styled(marker, Style::default().fg(theme.border_cyan)),
                Span::styled(format!(" {:24}", field.key), key_style),
                Span::styled(
                    format!("{:2}", dirty_suffix),
                    Style::default().fg(theme.border_yellow),
                ),
                Span::styled(display_value, value_style),
            ]);
            if is_selected && field_entered {
                row = row.style(Style::default().bg(theme.panel_header_bg));
            }
            row
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(25),
            Constraint::Length(2),
            Constraint::Min(10),
        ],
    )
    .column_spacing(0);
    f.render_widget(table, area);
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
        FieldType::Model => 18,
        _ => 5,
    }
    .min(area.height.saturating_sub(4))
    .max(5);

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
            let input = Paragraph::new(input_line).style(Style::default().fg(theme.text_primary));
            f.render_widget(input, inner);

            let cursor_x = inner.x + 2 + dialog.cursor as u16;
            f.set_cursor_position((cursor_x.min(inner.x + inner.width - 1), inner.y));
        }
        FieldType::Password => {
            let masked = "*".repeat(dialog.input.len());
            let input_line = format!("> {}", masked);
            let input = Paragraph::new(input_line).style(Style::default().fg(theme.text_primary));
            f.render_widget(input, inner);

            let cursor_x = inner.x + 2 + dialog.cursor as u16;
            f.set_cursor_position((cursor_x.min(inner.x + inner.width - 1), inner.y));
        }
        FieldType::Toggle | FieldType::Select => {
            for (i, opt) in dialog.options.iter().enumerate() {
                let opt_area = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
                let marker = if i == dialog.selected {
                    "\u{25cf}"
                } else {
                    "\u{25cb}"
                };
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

        FieldType::Model => {
            let search_line = format!("Search: {}", dialog.input);
            let search = Paragraph::new(search_line).style(Style::default().fg(theme.text_primary));
            f.render_widget(search, Rect::new(inner.x, inner.y, inner.width, 1));

            let cursor_x = inner.x + 8 + dialog.cursor as u16;
            f.set_cursor_position((cursor_x.min(inner.x + inner.width - 1), inner.y));

            let list_area = Rect::new(
                inner.x,
                inner.y + 2,
                inner.width,
                inner.height.saturating_sub(2),
            );

            if dialog.model_loading {
                let msg = Paragraph::new("Loading cached models...")
                    .style(Style::default().fg(theme.text_dim));
                f.render_widget(msg, list_area);
            } else if let Some(ref err) = dialog.model_error {
                let msg = Paragraph::new(format!(
                    "No cached models available.\n\n{}\n\nRun: abbot providers refresh",
                    err
                ))
                .style(Style::default().fg(theme.error_fg))
                .wrap(Wrap { trim: false });
                f.render_widget(msg, list_area);
            } else {
                let idxs = dialog.model_filtered_indices();
                if idxs.is_empty() {
                    let msg =
                        Paragraph::new("(no matches)").style(Style::default().fg(theme.text_dim));
                    f.render_widget(msg, list_area);
                } else {
                    let visible_rows = list_area.height as usize;
                    let visible_rows = visible_rows.max(1);
                    let selected = dialog.selected.min(idxs.len().saturating_sub(1));
                    let start = selected.saturating_sub(visible_rows.saturating_sub(1));
                    let end = (start + visible_rows).min(idxs.len());

                    for (row_idx, opt_idx) in idxs[start..end].iter().enumerate() {
                        let is_sel = start + row_idx == selected;
                        let marker = if is_sel { "●" } else { " " };
                        let style = if is_sel {
                            Style::default()
                                .fg(theme.text_primary)
                                .bg(theme.panel_header_bg)
                        } else {
                            Style::default().fg(theme.text_dim)
                        };

                        let line = Line::from(vec![
                            Span::styled(
                                format!("{} ", marker),
                                Style::default().fg(theme.border_cyan),
                            ),
                            Span::styled(dialog.model_options[*opt_idx].1.clone(), style),
                        ]);

                        f.render_widget(
                            Paragraph::new(line),
                            Rect::new(
                                list_area.x,
                                list_area.y + row_idx as u16,
                                list_area.width,
                                1,
                            ),
                        );
                    }
                }
            }
        }
    }

    let hint_area = Rect::new(
        dialog_area.x + 2,
        dialog_area.y + dialog_area.height - 2,
        dialog_area.width.saturating_sub(4),
        1,
    );
    let hint_text = if dialog.field_type == FieldType::Model {
        "[Enter] Select  [Esc] Cancel"
    } else {
        "[Enter] Save  [Esc] Cancel"
    };
    let hint = Paragraph::new(hint_text).style(Style::default().fg(theme.text_dim));
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
    let hint = Paragraph::new("[Y] Yes  [N/Esc] No").style(Style::default().fg(theme.text_primary));
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

    draw_statusline(
        f,
        theme,
        area,
        left,
        "[^S Save] [^T] [^C]",
        theme.border_magenta,
    );
}
