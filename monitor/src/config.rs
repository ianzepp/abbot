//! Config editor -- section/field/dialog navigation and validation.
//!
//! Manages a three-level focus hierarchy (section -> field -> dialog) with
//! async config loading and saving via the daemon admin API.

// =============================================================================
// TYPES
// =============================================================================

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

/// Which level of the config editor has keyboard focus.
///
/// Transitions: Sections -> Fields (on Enter/Right) -> Dialog (on Enter).
/// Dialog -> Fields (on Esc/Enter) -> Sections (on Esc/Left).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConfigFocus {
    #[default]
    Sections,
    Fields,
    Dialog,
}

/// Controls which dialog variant is shown when editing a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    Password,
    Number,
    /// Boolean yes/no picker.
    Toggle,
    /// Fixed option list picker.
    Select,
}

/// Runtime value for a config field, used for display and dirty-checking.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Text(String),
    Number(f64),
    Bool(bool),
    /// Index into a fixed option list plus the options themselves.
    Selected(usize, Vec<String>),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumberKind {
    Float,
    IntSigned,
    IntUnsigned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonKind {
    Bool,
    String,
    Number(NumberKind),
    Object,
    Array,
    Null,
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

/// A single editable key-value pair within a config section.
///
/// Tracks both the current `value` and the `original` loaded from the server
/// so the UI can show dirty indicators and diff on save.
#[derive(Debug, Clone)]
pub struct ConfigField {
    pub key: String,
    pub field_type: FieldType,
    pub value: FieldValue,
    pub original: FieldValue,
    pub json_kind: JsonKind,
}

impl ConfigField {
    pub fn is_dirty(&self) -> bool {
        self.value != self.original
    }

    fn from_json(key: impl Into<String>, value: &serde_json::Value) -> Self {
        let key = key.into();
        let (field_type, val, json_kind) = match value {
            serde_json::Value::String(s) => (
                FieldType::Text,
                FieldValue::Text(s.clone()),
                JsonKind::String,
            ),
            serde_json::Value::Bool(b) => (FieldType::Toggle, FieldValue::Bool(*b), JsonKind::Bool),
            serde_json::Value::Number(n) => {
                let kind = if n.is_u64() {
                    NumberKind::IntUnsigned
                } else if n.is_i64() {
                    NumberKind::IntSigned
                } else {
                    NumberKind::Float
                };
                let as_f64 = n.as_f64().unwrap_or(0.0);
                (
                    FieldType::Number,
                    FieldValue::Number(as_f64),
                    JsonKind::Number(kind),
                )
            }
            serde_json::Value::Array(_) => (
                FieldType::Text,
                FieldValue::Text(value.to_string()),
                JsonKind::Array,
            ),
            serde_json::Value::Object(_) => (
                FieldType::Text,
                FieldValue::Text(value.to_string()),
                JsonKind::Object,
            ),
            serde_json::Value::Null => (FieldType::Text, FieldValue::None, JsonKind::Null),
        };
        Self {
            key,
            field_type,
            value: val.clone(),
            original: val,
            json_kind,
        }
    }

    pub fn password(key: impl Into<String>, value: Option<String>) -> Self {
        let val = FieldValue::Text(value.unwrap_or_default());
        Self {
            key: key.into(),
            field_type: FieldType::Password,
            value: val.clone(),
            original: val,
            json_kind: JsonKind::String,
        }
    }

    pub fn select(key: impl Into<String>, options: Vec<String>, selected: Option<usize>) -> Self {
        let val = FieldValue::Selected(selected.unwrap_or(0), options);
        Self {
            key: key.into(),
            field_type: FieldType::Select,
            value: val.clone(),
            original: val,
            json_kind: JsonKind::String,
        }
    }

    fn to_json_value(&self) -> Result<Option<serde_json::Value>, String> {
        match self.json_kind {
            JsonKind::Bool => match &self.value {
                FieldValue::Bool(b) => Ok(Some(serde_json::Value::Bool(*b))),
                FieldValue::None => Ok(None),
                _ => Ok(None),
            },
            JsonKind::String => match &self.value {
                FieldValue::Text(s) => {
                    if s.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(serde_json::Value::String(s.clone())))
                    }
                }
                FieldValue::None => Ok(None),
                _ => Ok(None),
            },
            JsonKind::Number(kind) => match &self.value {
                FieldValue::Number(n) => {
                    if !n.is_finite() {
                        return Err(format!("{} must be a finite number", self.key));
                    }
                    match kind {
                        NumberKind::Float => serde_json::Number::from_f64(*n)
                            .map(serde_json::Value::Number)
                            .ok_or_else(|| "invalid float value".to_string())
                            .map(Some),
                        NumberKind::IntSigned => {
                            if n.fract() != 0.0 {
                                return Err(format!("{} must be an integer", self.key));
                            }
                            if *n < i64::MIN as f64 || *n > i64::MAX as f64 {
                                return Err(format!(
                                    "{} must be in [{}, {}]",
                                    self.key,
                                    i64::MIN,
                                    i64::MAX
                                ));
                            }
                            Ok(Some(serde_json::Value::Number(serde_json::Number::from(
                                *n as i64,
                            ))))
                        }
                        NumberKind::IntUnsigned => {
                            if n.fract() != 0.0 || *n < 0.0 {
                                return Err(format!("{} must be a non-negative integer", self.key));
                            }
                            if *n > u64::MAX as f64 {
                                return Err(format!("{} must be <= {}", self.key, u64::MAX));
                            }
                            Ok(Some(serde_json::Value::Number(serde_json::Number::from(
                                *n as u64,
                            ))))
                        }
                    }
                }
                FieldValue::None => Ok(None),
                _ => Ok(None),
            },
            JsonKind::Object | JsonKind::Array => match &self.value {
                FieldValue::Text(s) => {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        return Ok(None);
                    }
                    let parsed: serde_json::Value = serde_json::from_str(trimmed)
                        .map_err(|e| format!("{} must be valid JSON: {}", self.key, e))?;
                    match (self.json_kind, &parsed) {
                        (JsonKind::Object, serde_json::Value::Object(_)) => Ok(Some(parsed)),
                        (JsonKind::Array, serde_json::Value::Array(_)) => Ok(Some(parsed)),
                        (JsonKind::Object, _) => Err(format!("{} must be a JSON object", self.key)),
                        (JsonKind::Array, _) => Err(format!("{} must be a JSON array", self.key)),
                        _ => Ok(Some(parsed)),
                    }
                }
                FieldValue::None => Ok(None),
                _ => Ok(None),
            },
            JsonKind::Null => match &self.value {
                FieldValue::Text(s) => {
                    if s.trim().is_empty() {
                        Ok(None)
                    } else if s.trim() == "null" {
                        Ok(Some(serde_json::Value::Null))
                    } else {
                        Ok(Some(serde_json::Value::String(s.clone())))
                    }
                }
                FieldValue::None => Ok(None),
                _ => Ok(None),
            },
        }
    }
}

/// A named group of config fields (e.g. "head", "hand", "server").
#[derive(Debug, Clone)]
pub struct ConfigSection {
    pub name: String,
    pub fields: Vec<ConfigField>,
    pub is_scalar: bool,
}

impl ConfigSection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            fields: Vec::new(),
            is_scalar: false,
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

/// Transient editing state for the focused field's inline dialog.
///
/// Created by `ConfigDialog::for_field()` when the user presses Enter on a field,
/// and consumed back into `FieldValue` via `to_value()` on confirm.
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
    /// Build a dialog for the given field.
    pub fn for_field(field: &ConfigField) -> Self {
        let (input, selected, options) = match &field.value {
            FieldValue::Text(s) => (s.clone(), 0, Vec::new()),
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
        }
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
        }
    }
}

/// Top-level state for the config editor view.
///
/// Holds the section/field/dialog focus hierarchy, all parsed config sections,
/// and the original JSON blob for round-trip fidelity on save.
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
    /// Preserved so that `to_json()` can merge edits back without losing
    /// unknown keys that the TUI doesn't expose as fields.
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

    /// Validates numeric fields against their expected daemon types before save.
    ///
    /// WHY: All numbers are stored as f64 in FieldValue, but the daemon config
    /// expects specific integer types (u32, u64, usize). We catch overflows and
    /// fractional values here to give clear error messages instead of silent
    /// truncation or deserialization failures on the daemon side.
    pub fn validate_for_save(&self) -> Result<(), String> {
        for section in &self.sections {
            for field in &section.fields {
                if let Err(err) = field.to_json_value() {
                    let name = if section.is_scalar {
                        section.name.clone()
                    } else {
                        format!("{}.{}", section.name, field.key)
                    };
                    return Err(format!("{name}: {err}"));
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
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            for key in keys {
                if let Some(value) = obj.get(key) {
                    let section = build_section_from_json(key, value);
                    self.sections.push(section);
                }
            }
        }

        self.selected_section = 0;
        self.selected_field = 0;
        self.focus = ConfigFocus::Sections;
    }

    pub fn to_json(&self) -> Result<serde_json::Value, String> {
        let mut obj = self
            .base_json
            .as_object()
            .cloned()
            .unwrap_or_else(serde_json::Map::new);

        for section in &self.sections {
            if section.is_scalar {
                let field = section.fields.first();
                let val = match field {
                    Some(f) => f.to_json_value()?,
                    None => None,
                };
                if let Some(v) = val {
                    obj.insert(section.name.clone(), v);
                } else {
                    obj.remove(&section.name);
                }
                continue;
            }

            let mut section_obj = obj
                .get(&section.name)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_else(serde_json::Map::new);
            for field in &section.fields {
                let json_val = field.to_json_value()?;
                if let Some(val) = json_val {
                    set_json_path(&mut section_obj, &field.key, val);
                } else {
                    remove_json_path(&mut section_obj, &field.key);
                }
            }
            if !section_obj.is_empty() {
                obj.insert(section.name.clone(), serde_json::Value::Object(section_obj));
            } else {
                obj.remove(&section.name);
            }
        }

        Ok(serde_json::Value::Object(obj))
    }
}

/// Expected Rust type for a numeric config field on the daemon side.
#[derive(Debug, Clone, Copy)]
enum NumericKind {
    Float,
    U32,
    U64,
    Usize,
}

/// Maps (section, key) pairs to their daemon-side numeric types for validation.
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
        ("harness", "slow_idle") => Some(NumericKind::U64),
        ("harness", "deep_idle") => Some(NumericKind::U64),

        ("head", "pool") => Some(NumericKind::Usize),
        ("hand", "max_iters") => Some(NumericKind::Usize),
        ("hand", "pool") => Some(NumericKind::Usize),

        _ => None,
    }
}

fn build_section_from_json(name: &str, value: &serde_json::Value) -> ConfigSection {
    match value {
        serde_json::Value::Object(map) => {
            let mut section = ConfigSection::new(name);
            let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut fields = Vec::new();
            for (key, val) in entries {
                flatten_json(key, val, &mut fields);
            }
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            for (path, val) in fields {
                section.fields.push(ConfigField::from_json(path, &val));
            }
            section
        }
        _ => {
            let mut section = ConfigSection::new(name);
            section.is_scalar = true;
            section.fields.push(ConfigField::from_json("value", value));
            section
        }
    }
}

fn flatten_json(
    prefix: &str,
    value: &serde_json::Value,
    out: &mut Vec<(String, serde_json::Value)>,
) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                if let Some(val) = map.get(key) {
                    let path = format!("{}.{}", prefix, key);
                    flatten_json(&path, val, out);
                }
            }
        }
        _ => {
            out.push((prefix.to_string(), value.clone()));
        }
    }
}

fn set_json_path(
    map: &mut serde_json::Map<String, serde_json::Value>,
    path: &str,
    value: serde_json::Value,
) {
    let parts: Vec<&str> = path.split('.').collect();
    set_json_path_parts(map, &parts, value);
}

fn set_json_path_parts(
    map: &mut serde_json::Map<String, serde_json::Value>,
    parts: &[&str],
    value: serde_json::Value,
) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        map.insert(parts[0].to_string(), value);
        return;
    }
    let entry = map
        .entry(parts[0].to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if !matches!(entry, serde_json::Value::Object(_)) {
        *entry = serde_json::Value::Object(serde_json::Map::new());
    }
    if let serde_json::Value::Object(next) = entry {
        set_json_path_parts(next, &parts[1..], value);
    }
}

fn remove_json_path(map: &mut serde_json::Map<String, serde_json::Value>, path: &str) {
    let parts: Vec<&str> = path.split('.').collect();
    remove_json_path_parts(map, &parts);
}

fn remove_json_path_parts(map: &mut serde_json::Map<String, serde_json::Value>, parts: &[&str]) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        map.remove(parts[0]);
        return;
    }
    let Some(next_val) = map.get_mut(parts[0]) else {
        return;
    };
    if let serde_json::Value::Object(next_map) = next_val {
        remove_json_path_parts(next_map, &parts[1..]);
        if next_map.is_empty() {
            map.remove(parts[0]);
        }
    }
}

// =============================================================================
// DRAWING
// =============================================================================

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

    // WHY: When focus moves into Fields or Dialog, the selected section row
    // gets a subtle background highlight so the user can still see which
    // section they're editing without the active marker bullet.
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
                            // WHY: Cap at 16 asterisks so long API keys don't
                            // blow out the field column width.
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
    }

    let hint_area = Rect::new(
        dialog_area.x + 2,
        dialog_area.y + dialog_area.height - 2,
        dialog_area.width.saturating_sub(4),
        1,
    );
    let hint =
        Paragraph::new("[Enter] Save  [Esc] Cancel").style(Style::default().fg(theme.text_dim));
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
