#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LtmAction {
    Append(String),
    Replace { pattern: String, content: String },
    Clear(String),
}

#[derive(Debug, Clone, Default)]
pub struct ParsedHeartResponse {
    pub actions: Vec<LtmAction>,
}

impl ParsedHeartResponse {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

pub fn parse_heart_response(response: &str) -> ParsedHeartResponse {
    ParsedHeartResponse {
        actions: parse_ltm_blocks(response),
    }
}

fn parse_ltm_blocks(text: &str) -> Vec<LtmAction> {
    let mut actions = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("--- ltm ") {
        let header_start = start + 8;
        let after_marker = &remaining[header_start..];

        let Some(header_end) = after_marker.find(" ---") else {
            break;
        };
        let header = after_marker[..header_end].trim();

        let content_start = header_end + 4;
        let content_region = &after_marker[content_start..];

        let Some(end_marker) = content_region.find("--- end ---") else {
            break;
        };

        let content = content_region[..end_marker].trim().to_string();

        if let Some(action) = parse_ltm_action(header, &content) {
            actions.push(action);
        }

        let total_consumed = header_start + content_start + end_marker + 11;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    actions
}

fn parse_ltm_action(header: &str, content: &str) -> Option<LtmAction> {
    if header == "append" {
        return Some(LtmAction::Append(content.to_string()));
    }

    if let Some(rest) = header.strip_prefix("replace ") {
        if let Some(pattern) = parse_quoted_string(rest.trim()) {
            return Some(LtmAction::Replace {
                pattern,
                content: content.to_string(),
            });
        }
    }

    if let Some(rest) = header.strip_prefix("clear ") {
        if let Some(pattern) = parse_quoted_string(rest.trim()) {
            return Some(LtmAction::Clear(pattern));
        }
    }

    None
}

fn parse_quoted_string(s: &str) -> Option<String> {
    if s.starts_with('"') && s.len() > 1 {
        if let Some(end) = s[1..].find('"') {
            return Some(s[1..end + 1].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_append() {
        let r = r#"
Some reflection here.

--- ltm append ---
Curious about: Rust error handling patterns.
--- end ---
"#;
        let parsed = parse_heart_response(r);
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(
            parsed.actions[0],
            LtmAction::Append("Curious about: Rust error handling patterns.".to_string())
        );
    }

    #[test]
    fn parses_replace() {
        let r = r#"
--- ltm replace "old interest" ---
New interest replaces the old one.
--- end ---
"#;
        let parsed = parse_heart_response(r);
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(
            parsed.actions[0],
            LtmAction::Replace {
                pattern: "old interest".to_string(),
                content: "New interest replaces the old one.".to_string(),
            }
        );
    }

    #[test]
    fn parses_clear() {
        let r = r#"
--- ltm clear "stale note" ---
--- end ---
"#;
        let parsed = parse_heart_response(r);
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(
            parsed.actions[0],
            LtmAction::Clear("stale note".to_string())
        );
    }

    #[test]
    fn parses_multiple_actions() {
        let r = r#"
The head has been working on Rust projects.

--- ltm append ---
Remember: Follow up on async debugging.
--- end ---

--- ltm append ---
Curious about: Error handling patterns.
--- end ---

--- ltm clear "Python projects" ---
--- end ---
"#;
        let parsed = parse_heart_response(r);
        assert_eq!(parsed.actions.len(), 3);
        assert!(matches!(&parsed.actions[0], LtmAction::Append(s) if s.contains("Follow up")));
        assert!(matches!(&parsed.actions[1], LtmAction::Append(s) if s.contains("Error handling")));
        assert!(matches!(&parsed.actions[2], LtmAction::Clear(s) if s.contains("Python")));
    }

    #[test]
    fn empty_response() {
        let r = "just reflection, no actions";
        let parsed = parse_heart_response(r);
        assert!(parsed.is_empty());
    }

    #[test]
    fn multiline_append() {
        let r = r#"
--- ltm append ---
Observations from today:
- User prefers concise answers
- Rust projects are the focus
- Follow up needed on testing
--- end ---
"#;
        let parsed = parse_heart_response(r);
        assert_eq!(parsed.actions.len(), 1);
        if let LtmAction::Append(content) = &parsed.actions[0] {
            assert!(content.contains("Observations from today:"));
            assert!(content.contains("- User prefers concise answers"));
        } else {
            panic!("Expected Append");
        }
    }
}
