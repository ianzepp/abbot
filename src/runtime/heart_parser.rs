use super::parser::{parse_fenced_blocks, parse_quoted};

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
    let blocks = parse_fenced_blocks(response);

    let actions = blocks
        .into_iter()
        .filter(|b| b.tag == "ltm")
        .filter_map(|b| parse_ltm_action(&b.header, &b.content))
        .collect();

    ParsedHeartResponse { actions }
}

fn parse_ltm_action(header: &str, content: &str) -> Option<LtmAction> {
    if header == "append" {
        return Some(LtmAction::Append(content.to_string()));
    }

    if let Some(rest) = header.strip_prefix("replace ") {
        if let Some(pattern) = parse_quoted(rest.trim()) {
            return Some(LtmAction::Replace {
                pattern,
                content: content.to_string(),
            });
        }
    }

    if let Some(rest) = header.strip_prefix("clear ") {
        if let Some(pattern) = parse_quoted(rest.trim()) {
            return Some(LtmAction::Clear(pattern));
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

```ltm append
Curious about: Rust error handling patterns.
```
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
```ltm replace "old interest"
New interest replaces the old one.
```
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
```ltm clear "stale note"
```
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

```ltm append
Remember: Follow up on async debugging.
```

```ltm append
Curious about: Error handling patterns.
```

```ltm clear "Python projects"
```
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
```ltm append
Observations from today:
- User prefers concise answers
- Rust projects are the focus
- Follow up needed on testing
```
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
