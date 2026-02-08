use uuid::Uuid;

use serde_json::Value;

/// Context for the need currently being processed.
///
/// Simplified after room refactor: LLM transcript, external tool handling,
/// and resume state are now managed by the room runner and Door.
#[derive(Debug, Clone)]
pub(super) struct ActiveNeed {
    pub(super) need_id: String,
    pub(super) need_text: String,
    pub(super) context: String,
    pub(super) scope: Option<String>,
    pub(super) reply_to: Option<Uuid>,
}

impl ActiveNeed {
    pub(crate) fn try_from_value(value: &Value) -> Option<Self> {
        let need_id = value
            .get("need_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let need_text = value
            .get("need")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let context = value
            .get("context")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let scope = value
            .get("scope")
            .and_then(|x| x.as_str())
            .unwrap_or("main")
            .to_string();
        let reply_to = value
            .get("reply_to")
            .and_then(|x| x.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        if need_id.trim().is_empty() || need_text.trim().is_empty() {
            return None;
        }

        Some(Self {
            need_id,
            need_text,
            context,
            scope: Some(scope),
            reply_to,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ActiveNeed;
    use serde_json::json;

    #[test]
    fn parses_minimal_need() {
        let input = json!({
            "need_id": "n1",
            "need": "do thing",
        });
        let parsed = ActiveNeed::try_from_value(&input).expect("expected parsed need");
        assert_eq!(parsed.need_id, "n1");
        assert_eq!(parsed.need_text, "do thing");
        assert_eq!(parsed.context, "");
        assert_eq!(parsed.scope.as_deref(), Some("main"));
        assert_eq!(parsed.reply_to, None);
    }

    #[test]
    fn rejects_empty_fields() {
        let input = json!({
            "need_id": "",
            "need": "do thing",
        });
        assert!(ActiveNeed::try_from_value(&input).is_none());
    }

    #[test]
    fn parses_reply_to_uuid() {
        let input = json!({
            "need_id": "n2",
            "need": "do thing",
            "reply_to": "00000000-0000-0000-0000-000000000000"
        });
        let parsed = ActiveNeed::try_from_value(&input).expect("expected parsed need");
        assert!(parsed.reply_to.is_some());
    }
}
