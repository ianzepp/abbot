/// Poverty mode controls token budget behavior.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PovertyMode {
    /// No poverty overlay.
    #[default]
    None,
    /// 47 tokens left. This is the end.
    Destitute,
    /// Terse. One tool call. No exploration.
    Scraping,
    /// Efficient but not desperate.
    Frugal,
    /// Normal operation.
    Comfortable,
    /// Thorough. Multiple approaches. Explains reasoning.
    Flush,
    /// Token printer go brrr.
    Bezos,
}

impl PovertyMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "" | "none" | "off" => Some(Self::None),
            "destitute" | "broke" => Some(Self::Destitute),
            "scraping" | "poor" => Some(Self::Scraping),
            "frugal" | "thrifty" => Some(Self::Frugal),
            "comfortable" | "normal" => Some(Self::Comfortable),
            "flush" | "rich" => Some(Self::Flush),
            "bezos" | "infinite" => Some(Self::Bezos),
            _ => None,
        }
    }
}
