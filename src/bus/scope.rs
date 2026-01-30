// Scope defines the routing destination for messages.
//
// Scopes are plain strings with conventional prefixes:
// - "main" - shared world scope
// - "head/<id>/mail" - private inbox for a head
// - "head/<id>/stm" - short-term memory
// - "head/<id>/ltm" - long-term memory
// - "task/<id>" - isolated thread for task execution

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope(String);

impl Scope {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn main() -> Self {
        Self("main".to_string())
    }

    pub fn head_mail(head_id: &str) -> Self {
        Self(format!("head/{}/mail", head_id))
    }

    pub fn head_stm(head_id: &str) -> Self {
        Self(format!("head/{}/stm", head_id))
    }

    pub fn head_ltm(head_id: &str) -> Self {
        Self(format!("head/{}/ltm", head_id))
    }

    pub fn task(task_id: &str) -> Self {
        Self(format!("task/{}", task_id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_main(&self) -> bool {
        self.0 == "main"
    }

    pub fn is_head_mail(&self) -> bool {
        self.0.starts_with("head/") && self.0.ends_with("/mail")
    }

    pub fn is_head_stm(&self) -> bool {
        self.0.starts_with("head/") && self.0.ends_with("/stm")
    }

    pub fn is_head_ltm(&self) -> bool {
        self.0.starts_with("head/") && self.0.ends_with("/ltm")
    }

    pub fn is_task(&self) -> bool {
        self.0.starts_with("task/")
    }

    pub fn head_id(&self) -> Option<&str> {
        if self.0.starts_with("head/") {
            let rest = &self.0[5..];
            rest.find('/').map(|i| &rest[..i])
        } else {
            None
        }
    }

    pub fn task_id(&self) -> Option<&str> {
        self.0.strip_prefix("task/")
    }

    /// Returns the scope kind for database storage (backwards compatibility).
    pub fn kind_str(&self) -> &str {
        if self.is_main() {
            "main"
        } else if self.0.starts_with("head/") {
            "head"
        } else if self.0.starts_with("task/") {
            "task"
        } else {
            "other"
        }
    }

    /// Returns the scope key for database storage (backwards compatibility).
    pub fn key(&self) -> &str {
        if self.is_main() {
            "main"
        } else if let Some(rest) = self.0.strip_prefix("head/") {
            rest
        } else if let Some(rest) = self.0.strip_prefix("task/") {
            rest
        } else {
            &self.0
        }
    }

    /// Reconstruct scope from kind and key (for loading from database).
    pub fn from_parts(kind: &str, key: &str) -> Self {
        match kind {
            "main" => Self::main(),
            "head" => Self(format!("head/{}", key)),
            "task" => Self(format!("task/{}", key)),
            "channel" => Self::main(), // legacy: map old #channels to main
            "mail" => Self(format!("head/{}/mail", key)), // legacy: map old @mail to head/x/mail
            _ => Self(key.to_string()),
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for Scope {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for Scope {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_scope() {
        let s = Scope::main();
        assert!(s.is_main());
        assert_eq!(s.to_string(), "main");
    }

    #[test]
    fn test_head_mail_scope() {
        let s = Scope::head_mail("Monk");
        assert!(s.is_head_mail());
        assert_eq!(s.head_id(), Some("Monk"));
        assert_eq!(s.to_string(), "head/Monk/mail");
    }

    #[test]
    fn test_task_scope() {
        let s = Scope::task("abc123");
        assert!(s.is_task());
        assert_eq!(s.task_id(), Some("abc123"));
        assert_eq!(s.to_string(), "task/abc123");
    }

    #[test]
    fn test_from_str() {
        let s: Scope = "main".into();
        assert!(s.is_main());

        let s: Scope = "head/Bot/mail".into();
        assert!(s.is_head_mail());
        assert_eq!(s.head_id(), Some("Bot"));
    }
}
