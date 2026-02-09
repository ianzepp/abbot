//! Color theme definitions for dark and light terminal modes.

use ratatui::style::Color;

/// Semantic color palette resolved from the active terminal mode.
#[derive(Clone)]
pub struct Theme {
    pub header_bg: Color,
    #[allow(dead_code)]
    pub panel_bg: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_dim: Color,
    pub border_green: Color,
    pub border_red: Color,
    pub border_yellow: Color,
    pub border_cyan: Color,
    pub accent: Color,
    pub code_fg: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            header_bg: Color::Rgb(40, 40, 40),
            panel_bg: Color::Rgb(30, 30, 30),
            text_primary: Color::White,
            text_secondary: Color::Rgb(180, 180, 180),
            text_dim: Color::DarkGray,
            border_green: Color::Green,
            border_red: Color::Red,
            border_yellow: Color::Yellow,
            border_cyan: Color::Cyan,
            accent: Color::Cyan,
            code_fg: Color::Rgb(200, 160, 80),
        }
    }

    pub fn light() -> Self {
        Self {
            header_bg: Color::Rgb(220, 220, 220),
            panel_bg: Color::Rgb(240, 240, 240),
            text_primary: Color::Rgb(30, 30, 30),
            text_secondary: Color::Rgb(60, 60, 60),
            text_dim: Color::Rgb(120, 120, 120),
            border_green: Color::Rgb(40, 140, 40),
            border_red: Color::Rgb(180, 40, 40),
            border_yellow: Color::Rgb(180, 140, 0),
            border_cyan: Color::Rgb(0, 140, 160),
            accent: Color::Rgb(0, 140, 160),
            code_fg: Color::Rgb(160, 80, 0),
        }
    }

    pub fn for_mode(dark_mode: bool) -> Self {
        if dark_mode {
            Self::dark()
        } else {
            Self::light()
        }
    }
}
