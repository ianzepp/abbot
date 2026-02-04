/// Tact mode controls how directly the model communicates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TactMode {
    /// No tact overlay.
    #[default]
    None,
    /// Be blunt and direct.
    Blunt,
    /// Be tactful and diplomatic.
    Tactful,
}

impl TactMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "" | "none" | "off" => Some(Self::None),
            "blunt" | "low" => Some(Self::Blunt),
            "tact" | "tactful" | "high" | "on" => Some(Self::Tactful),
            _ => None,
        }
    }
}
