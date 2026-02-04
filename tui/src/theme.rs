use ratatui::style::Color;

#[derive(Clone)]
pub struct Theme {
    pub header_bg: Color,
    pub panel_header_bg: Color,
    pub panel_bg: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_dim: Color,
    pub border_red: Color,
    pub border_blue: Color,
    pub border_green: Color,
    pub border_yellow: Color,
    pub border_magenta: Color,
    pub border_cyan: Color,
    pub selection: Color,
    pub status_bar_bg: Color,
    pub error_fg: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            header_bg: Color::Rgb(40, 40, 40),
            panel_header_bg: Color::Rgb(50, 50, 50),
            panel_bg: Color::Rgb(30, 30, 30),
            text_primary: Color::White,
            text_secondary: Color::Rgb(180, 180, 180),
            text_dim: Color::DarkGray,
            border_red: Color::Red,
            border_blue: Color::Blue,
            border_green: Color::Green,
            border_yellow: Color::Yellow,
            border_magenta: Color::Magenta,
            border_cyan: Color::Cyan,
            selection: Color::Green,
            status_bar_bg: Color::Blue,
            error_fg: Color::Red,
        }
    }

    pub fn light() -> Self {
        Self {
            header_bg: Color::Rgb(220, 220, 220),
            panel_header_bg: Color::Rgb(200, 200, 200),
            panel_bg: Color::Rgb(240, 240, 240),
            text_primary: Color::Rgb(30, 30, 30),
            text_secondary: Color::Rgb(60, 60, 60),
            text_dim: Color::Rgb(120, 120, 120),
            border_red: Color::Rgb(180, 40, 40),
            border_blue: Color::Rgb(40, 80, 180),
            border_green: Color::Rgb(40, 140, 40),
            border_yellow: Color::Rgb(180, 140, 0),
            border_magenta: Color::Rgb(140, 40, 140),
            border_cyan: Color::Rgb(0, 140, 160),
            selection: Color::Rgb(40, 140, 40),
            status_bar_bg: Color::Rgb(60, 100, 180),
            error_fg: Color::Rgb(180, 40, 40),
        }
    }

    pub fn for_mode(dark_mode: bool) -> Self {
        if dark_mode { Self::dark() } else { Self::light() }
    }
}
