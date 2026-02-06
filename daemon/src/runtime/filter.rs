/// Filter mode controls how much gets through the internal editor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FilterMode {
    /// No filter overlay.
    #[default]
    None,
    /// Everything goes through legal review. Maximum CYA.
    Hr,
    /// Professional personal brand. Thought leadership.
    LinkedIn,
    /// Casual work chat. Professional but human.
    Slack,
    /// Gaming server energy. Irreverent. Meme-literate.
    Discord,
    /// Anonymous board energy. Nothing sacred.
    Anon,
    /// Says things that would get accounts suspended.
    Banned,
}

impl FilterMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "" | "none" | "off" => Some(Self::None),
            "hr" | "corporate" | "legal" => Some(Self::Hr),
            "linkedin" | "professional" => Some(Self::LinkedIn),
            "slack" | "casual" => Some(Self::Slack),
            "discord" | "gamer" => Some(Self::Discord),
            "anon" | "anonymous" | "4chan" => Some(Self::Anon),
            "banned" | "unhinged" => Some(Self::Banned),
            _ => None,
        }
    }
}
