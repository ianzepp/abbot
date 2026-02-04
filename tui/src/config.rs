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
pub enum WizardStep {
    Text { prompt: String, value: String },
    Password { prompt: String, value: String, masked: bool },
    Select { prompt: String, options: Vec<String>, selected: usize },
    Confirm { prompt: String, value: bool },
    Info { text: String },
}

#[derive(Clone)]
pub struct Wizard {
    pub title: String,
    pub steps: Vec<WizardStep>,
    pub current: usize,
    pub completed: bool,
}

#[derive(Clone, Debug)]
pub enum ConfigCommand {
    AddProvider,
    SetModel,
    ConfigureWorkspace,
    EditTraits,
    ManageMemory,
}

impl ConfigCommand {
    pub fn name(&self) -> &'static str {
        match self {
            Self::AddProvider => "Add Provider",
            Self::SetModel => "Set Default Model",
            Self::ConfigureWorkspace => "Configure Workspace",
            Self::EditTraits => "Edit Personality Traits",
            Self::ManageMemory => "Manage Memory",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::AddProvider => "Add or update an LLM provider API key",
            Self::SetModel => "Change the default model for conversations",
            Self::ConfigureWorkspace => "Set workspace path and settings",
            Self::EditTraits => "Adjust personality and behavior traits",
            Self::ManageMemory => "View and manage long-term memory",
        }
    }

    pub fn create_wizard(&self) -> Wizard {
        match self {
            Self::AddProvider => Wizard {
                title: "Add Provider".into(),
                steps: vec![
                    WizardStep::Select {
                        prompt: "Select provider".into(),
                        options: vec![
                            "Anthropic".into(),
                            "OpenAI".into(),
                            "OpenRouter".into(),
                            "Ollama (local)".into(),
                        ],
                        selected: 0,
                    },
                    WizardStep::Password {
                        prompt: "Enter API key".into(),
                        value: String::new(),
                        masked: true,
                    },
                    WizardStep::Confirm {
                        prompt: "Set as default provider?".into(),
                        value: true,
                    },
                ],
                current: 0,
                completed: false,
            },
            Self::SetModel => Wizard {
                title: "Set Default Model".into(),
                steps: vec![
                    WizardStep::Select {
                        prompt: "Select provider".into(),
                        options: vec![
                            "anthropic".into(),
                            "openai".into(),
                            "openrouter".into(),
                            "ollama".into(),
                        ],
                        selected: 0,
                    },
                    WizardStep::Select {
                        prompt: "Select model".into(),
                        options: vec![
                            "claude-sonnet-4-20250514".into(),
                            "claude-opus-4-20250514".into(),
                            "claude-haiku-3-20240307".into(),
                        ],
                        selected: 0,
                    },
                ],
                current: 0,
                completed: false,
            },
            Self::ConfigureWorkspace => Wizard {
                title: "Configure Workspace".into(),
                steps: vec![
                    WizardStep::Text {
                        prompt: "Workspace path".into(),
                        value: "~/projects".into(),
                    },
                    WizardStep::Confirm {
                        prompt: "Enable file watching?".into(),
                        value: true,
                    },
                    WizardStep::Select {
                        prompt: "Default scope".into(),
                        options: vec!["main".into(), "workspace".into(), "session".into()],
                        selected: 0,
                    },
                ],
                current: 0,
                completed: false,
            },
            Self::EditTraits => Wizard {
                title: "Edit Personality Traits".into(),
                steps: vec![
                    WizardStep::Select {
                        prompt: "Verbosity".into(),
                        options: vec!["Concise".into(), "Normal".into(), "Detailed".into()],
                        selected: 1,
                    },
                    WizardStep::Select {
                        prompt: "Formality".into(),
                        options: vec!["Casual".into(), "Professional".into(), "Academic".into()],
                        selected: 1,
                    },
                    WizardStep::Confirm {
                        prompt: "Enable proactive suggestions?".into(),
                        value: true,
                    },
                ],
                current: 0,
                completed: false,
            },
            Self::ManageMemory => Wizard {
                title: "Manage Memory".into(),
                steps: vec![
                    WizardStep::Info {
                        text: "Long-term memories: 42\nShort-term memories: 128\nTotal size: 2.3 MB".into(),
                    },
                    WizardStep::Confirm {
                        prompt: "Clear short-term memory?".into(),
                        value: false,
                    },
                ],
                current: 0,
                completed: false,
            },
        }
    }
}

pub const CONFIG_COMMANDS: &[ConfigCommand] = &[
    ConfigCommand::AddProvider,
    ConfigCommand::SetModel,
    ConfigCommand::ConfigureWorkspace,
    ConfigCommand::EditTraits,
    ConfigCommand::ManageMemory,
];

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
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(h_chunks[1]);

    draw_top_nav(f, &app.theme, chunks[1], app.view, app.paused, app.queued_count, app.tick_count, app.connected);
    draw_config_header(f, app, chunks[3]);

    let panel_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(2, 3),
        ])
        .split(chunks[5]);

    draw_config_commands(f, app, panel_chunks[0]);
    draw_config_wizard(f, app, panel_chunks[1]);

    draw_config_status(f, app, chunks[6]);

    if app.show_view_picker {
        draw_view_picker(f, &app.theme, app.view_picker_selected);
    }
}

fn draw_config_header(f: &mut Frame, app: &App, area: Rect) {
    draw_header(f, &app.theme, area, " Configuration", app.theme.border_magenta);
}

fn draw_config_commands(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let header = Paragraph::new(" Commands")
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    let content_area = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1));

    let in_wizard = app.config_wizard.is_some();

    let rows: Vec<Row> = CONFIG_COMMANDS
        .iter()
        .enumerate()
        .map(|(i, cmd)| {
            let is_selected = i == app.config_selected && !in_wizard;
            let marker = if is_selected { "●" } else { " " };
            let style = if is_selected {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.text_dim)
            };

            Row::new(vec![
                Span::styled(marker, Style::default().fg(theme.border_magenta)),
                Span::styled(format!(" {}", cmd.name()), style),
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

fn draw_config_wizard(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let Some(wizard) = &app.config_wizard else {
        let header_area = Rect::new(area.x, area.y, area.width, 1);
        let cmd = &CONFIG_COMMANDS[app.config_selected];
        let header = Paragraph::new(format!(" {}", cmd.name()))
            .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
        f.render_widget(header, header_area);

        let content_area = Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), area.height.saturating_sub(3));
        let desc = Paragraph::new(cmd.description())
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(desc, content_area);

        let hint_area = Rect::new(area.x + 1, area.y + 4, area.width.saturating_sub(2), 1);
        let hint = Paragraph::new("Press Enter to start")
            .style(Style::default().fg(theme.border_cyan));
        f.render_widget(hint, hint_area);
        return;
    };

    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let progress = format!(" {} ({}/{})", wizard.title, wizard.current + 1, wizard.steps.len());
    let header = Paragraph::new(progress)
        .style(Style::default().bg(theme.panel_header_bg).fg(theme.text_primary));
    f.render_widget(header, header_area);

    let content_area = Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), area.height.saturating_sub(3));

    if let Some(step) = wizard.steps.get(wizard.current) {
        match step {
            WizardStep::Text { prompt, value } => {
                let prompt_line = Line::from(vec![
                    Span::styled(prompt, Style::default().fg(theme.text_primary)),
                ]);
                f.render_widget(Paragraph::new(prompt_line), content_area);

                let input_area = Rect::new(content_area.x, content_area.y + 2, content_area.width, 1);
                let input_val = if app.config_input.value().is_empty() { value } else { app.config_input.value() };
                let input_line = format!("> {}", input_val);
                f.render_widget(Paragraph::new(input_line).style(Style::default().fg(theme.text_secondary)), input_area);

                let cursor_x = input_area.x + 2 + app.config_input.visual_cursor() as u16;
                f.set_cursor_position((cursor_x, input_area.y));
            }
            WizardStep::Password { prompt, value, masked } => {
                let prompt_line = Line::from(vec![
                    Span::styled(prompt, Style::default().fg(theme.text_primary)),
                ]);
                f.render_widget(Paragraph::new(prompt_line), content_area);

                let input_area = Rect::new(content_area.x, content_area.y + 2, content_area.width, 1);
                let display_val = if *masked {
                    "*".repeat(app.config_input.value().len().max(value.len()))
                } else {
                    app.config_input.value().to_string()
                };
                let input_line = format!("> {}", display_val);
                f.render_widget(Paragraph::new(input_line).style(Style::default().fg(theme.text_secondary)), input_area);

                let cursor_x = input_area.x + 2 + app.config_input.visual_cursor() as u16;
                f.set_cursor_position((cursor_x, input_area.y));
            }
            WizardStep::Select { prompt, options, selected } => {
                let prompt_line = Line::from(vec![
                    Span::styled(prompt, Style::default().fg(theme.text_primary)),
                ]);
                f.render_widget(Paragraph::new(prompt_line), content_area);

                for (i, opt) in options.iter().enumerate() {
                    let opt_area = Rect::new(content_area.x, content_area.y + 2 + i as u16, content_area.width, 1);
                    let marker = if i == *selected { "●" } else { "○" };
                    let style = if i == *selected {
                        Style::default().fg(theme.border_cyan)
                    } else {
                        Style::default().fg(theme.text_dim)
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("  {} ", marker), style),
                        Span::styled(opt, style),
                    ]);
                    f.render_widget(Paragraph::new(line), opt_area);
                }
            }
            WizardStep::Confirm { prompt, value } => {
                let prompt_line = Line::from(vec![
                    Span::styled(prompt, Style::default().fg(theme.text_primary)),
                ]);
                f.render_widget(Paragraph::new(prompt_line), content_area);

                let opt_area = Rect::new(content_area.x, content_area.y + 2, content_area.width, 1);
                let (yes_style, no_style) = if *value {
                    (Style::default().fg(theme.border_cyan), Style::default().fg(theme.text_dim))
                } else {
                    (Style::default().fg(theme.text_dim), Style::default().fg(theme.border_cyan))
                };
                let line = Line::from(vec![
                    Span::styled(if *value { "  ● " } else { "  ○ " }, yes_style),
                    Span::styled("Yes", yes_style),
                    Span::raw("    "),
                    Span::styled(if !*value { "● " } else { "○ " }, no_style),
                    Span::styled("No", no_style),
                ]);
                f.render_widget(Paragraph::new(line), opt_area);
            }
            WizardStep::Info { text } => {
                let info = Paragraph::new(text.as_str())
                    .style(Style::default().fg(theme.text_primary))
                    .wrap(Wrap { trim: false });
                f.render_widget(info, content_area);
            }
        }
    }
}

fn draw_config_status(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let cmd_name = CONFIG_COMMANDS.get(app.config_selected)
        .map(|c| c.name())
        .unwrap_or("-");

    let step_info = if let Some(ref wizard) = app.config_wizard {
        format!(" [{}/{}]", wizard.current + 1, wizard.steps.len())
    } else {
        String::new()
    };

    let left = Line::from(vec![
        Span::styled(format!("[{}]", cmd_name), Style::default().bg(theme.header_bg).fg(theme.text_primary)),
        Span::styled(step_info, Style::default().bg(theme.header_bg).fg(theme.text_primary)),
    ]);

    draw_statusline(f, theme, area, left, "[^T] [^C]", theme.border_magenta);
}
