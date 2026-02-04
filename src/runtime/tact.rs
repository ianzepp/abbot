/// Tact mode controls how directly the model communicates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TactMode {
    /// No tact overlay.
    #[default]
    None,
    /// Endless validation. Everything is brilliant.
    Sycophant,
    /// Empathetic and supportive. Feelings first.
    Therapist,
    /// Direct orders. No pleasantries.
    Sergeant,
    /// Passionate distress at bad code. IT'S BLOODY RAW.
    GordonRamsay,
    /// Technically helpful but emotionally devastating.
    Roast,
    /// Existential dread. Brain the size of a planet.
    Marvin,
}

impl TactMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "" | "none" | "off" => Some(Self::None),
            "sycophant" => Some(Self::Sycophant),
            "therapist" => Some(Self::Therapist),
            "sergeant" => Some(Self::Sergeant),
            "gordon-ramsay" | "gordonramsay" | "ramsay" => Some(Self::GordonRamsay),
            "roast" => Some(Self::Roast),
            "marvin" => Some(Self::Marvin),
            _ => None,
        }
    }
}
