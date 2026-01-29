use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    Channel(String),
    Mail(String),
    Task(String),
}

impl Scope {
    pub fn channel(name: impl Into<String>) -> Self {
        let name = name.into();
        Self::Channel(name.trim_start_matches('#').to_string())
    }

    pub fn mail(recipient: impl Into<String>) -> Self {
        let recipient = recipient.into();
        Self::Mail(recipient.trim_start_matches('@').to_string())
    }

    pub fn task(path: impl Into<String>) -> Self {
        let path = path.into();
        Self::Task(path.trim_start_matches('§').to_string())
    }

    pub fn key(&self) -> &str {
        match self {
            Scope::Channel(k) | Scope::Mail(k) | Scope::Task(k) => k,
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            Scope::Channel(_) => "channel",
            Scope::Mail(_) => "mail",
            Scope::Task(_) => "task",
        }
    }

    pub fn parse_lossy(s: &str) -> Self {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix('#') {
            return Scope::Channel(rest.to_string());
        }
        if let Some(rest) = s.strip_prefix('@') {
            return Scope::Mail(rest.to_string());
        }
        if let Some(rest) = s.strip_prefix('§') {
            return Scope::Task(rest.to_string());
        }
        Scope::Channel(s.to_string())
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Channel(name) => write!(f, "#{}", name),
            Scope::Mail(name) => write!(f, "@{}", name),
            Scope::Task(path) => write!(f, "§{}", path),
        }
    }
}

impl From<&str> for Scope {
    fn from(value: &str) -> Self {
        Scope::parse_lossy(value)
    }
}

impl From<String> for Scope {
    fn from(value: String) -> Self {
        Scope::parse_lossy(&value)
    }
}

