/// Parser for LLM responses according to the grammar specification.
///
/// Extracts actions from responses:
/// - `<say channel="...">...</say>` → Say action
/// - `<exec tool="...">...</exec>` → Exec action
/// - `<pong/>` → Pong flag
///
/// Text outside tags is considered "thought" and discarded.

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Say { channel: String, text: String },
    Exec {
        tool: String,
        reason: Option<String>,
        destructive: bool,
        content: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ParsedResponse {
    pub actions: Vec<Action>,
    pub has_pong: bool,
}

impl ParsedResponse {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty() && !self.has_pong
    }
}

/// Strip XML-like tags from text, leaving only plain content.
pub fn strip_tags(text: &str) -> String {
    let mut result = text.to_string();

    // Remove <exec>...</exec> blocks
    while let Some(start) = result.find("<exec") {
        if let Some(end) = result[start..].find("</exec>") {
            result = format!("{}{}", &result[..start], &result[start + end + 7..]);
        } else {
            break;
        }
    }

    // Remove <say>...</say> blocks
    while let Some(start) = result.find("<say") {
        if let Some(end) = result[start..].find("</say>") {
            result = format!("{}{}", &result[..start], &result[start + end + 6..]);
        } else {
            break;
        }
    }

    // Remove <pong/>
    result = result.replace("<pong/>", "");

    // Clean up whitespace
    result.trim().to_string()
}

/// Parse an LLM response into actions.
pub fn parse(response: &str) -> ParsedResponse {
    let mut result = ParsedResponse::default();

    // Check for <pong/>
    if response.contains("<pong/>") {
        result.has_pong = true;
    }

    // Parse <say channel="...">...</say>
    result.actions.extend(parse_say_tags(response));

    // Parse <exec tool="...">...</exec>
    result.actions.extend(parse_exec_tags(response));

    result
}

/// Parse all <say channel="...">...</say> tags.
fn parse_say_tags(text: &str) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("<say channel=\"") {
        let after_open = &remaining[start + 14..]; // skip `<say channel="`

        // Find the closing quote for channel
        let Some(quote_end) = after_open.find('"') else {
            break;
        };
        let channel = after_open[..quote_end].to_string();

        // Find the closing >
        let after_channel = &after_open[quote_end + 1..];
        let Some(bracket_end) = after_channel.find('>') else {
            break;
        };

        // Find </say>
        let content_start = &after_channel[bracket_end + 1..];
        let Some(close_tag) = content_start.find("</say>") else {
            break;
        };

        let text = content_start[..close_tag].to_string();

        actions.push(Action::Say { channel, text });

        // Move past this tag
        let total_consumed = start + 14 + quote_end + 1 + bracket_end + 1 + close_tag + 6;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    actions
}

/// Parse all <exec tool="...">...</exec> tags.
fn parse_exec_tags(text: &str) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("<exec ") {
        // Find the closing > of the opening tag
        let tag_start = &remaining[start..];
        let Some(bracket_end) = tag_start.find('>') else {
            break;
        };
        let opening_tag = &tag_start[..bracket_end];

        // Extract tool attribute (required)
        let tool = extract_attr(opening_tag, "tool").unwrap_or_default();
        if tool.is_empty() {
            // No tool attribute, skip this tag
            remaining = &remaining[start + 6..];
            continue;
        }

        // Extract optional attributes
        let reason = extract_attr(opening_tag, "reason");
        let destructive = extract_attr(opening_tag, "destructive")
            .map(|v| v == "true")
            .unwrap_or(false);

        // Find </exec>
        let content_start = &tag_start[bracket_end + 1..];
        let Some(close_tag) = content_start.find("</exec>") else {
            break;
        };

        let content = content_start[..close_tag].to_string();

        actions.push(Action::Exec { tool, reason, destructive, content });

        // Move past this tag
        let total_consumed = start + bracket_end + 1 + close_tag + 7;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    actions
}

/// Extract an attribute value from an XML-like opening tag.
fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let pattern = format!("{}=\"", name);
    let start = tag.find(&pattern)?;
    let after_eq = &tag[start + pattern.len()..];
    let end = after_eq.find('"')?;
    Some(after_eq[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        let result = parse("");
        assert!(result.is_empty());
        assert!(!result.has_pong);
    }

    #[test]
    fn test_parse_thought_only() {
        let result = parse("I'm thinking about what to do here...");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_pong() {
        let result = parse("Nothing to do.\n<pong/>");
        assert!(result.has_pong);
        assert!(result.actions.is_empty());
    }

    #[test]
    fn test_parse_say() {
        let result = parse(r##"<say channel="#general">Hello world</say>"##);
        assert_eq!(result.actions.len(), 1);
        assert!(matches!(
            &result.actions[0],
            Action::Say { channel, text } if channel == "#general" && text == "Hello world"
        ));
    }

    #[test]
    fn test_parse_exec() {
        let result = parse(r#"<exec tool="bash">ls -la</exec>"#);
        assert_eq!(result.actions.len(), 1);
        assert!(matches!(
            &result.actions[0],
            Action::Exec { tool, content, .. } if tool == "bash" && content == "ls -la"
        ));
    }

    #[test]
    fn test_parse_exec_multiline() {
        let response = r#"<exec tool="write">/tmp/test.txt
line 1
line 2
line 3</exec>"#;
        let result = parse(response);
        assert_eq!(result.actions.len(), 1);
        if let Action::Exec { tool, content, .. } = &result.actions[0] {
            assert_eq!(tool, "write");
            assert!(content.contains("line 1"));
            assert!(content.contains("line 2"));
            assert!(content.contains("line 3"));
        } else {
            panic!("expected Exec action");
        }
    }

    #[test]
    fn test_parse_multiple_actions() {
        let response = r##"Let me check that file and tell you.
<exec tool="read">/tmp/log.txt</exec>
<say channel="#general">Checking the log file...</say>"##;

        let result = parse(response);
        assert_eq!(result.actions.len(), 2);

        let has_read = result.actions.iter().any(|a| matches!(
            a,
            Action::Exec { tool, content, .. } if tool == "read" && content == "/tmp/log.txt"
        ));
        let has_say = result.actions.iter().any(|a| matches!(
            a,
            Action::Say { channel, text } if channel == "#general" && text == "Checking the log file..."
        ));

        assert!(has_read, "should have read action");
        assert!(has_say, "should have say action");
    }

    #[test]
    fn test_parse_parallel_exec() {
        let response = r#"I'll do both at once.
<exec tool="bash">git status</exec>
<exec tool="read">Cargo.toml</exec>
<exec tool="find">*.rs</exec>"#;

        let result = parse(response);
        assert_eq!(result.actions.len(), 3);
    }

    #[test]
    fn test_parse_with_pong_and_actions() {
        // Unusual but valid: pong with actions
        let response = r#"<exec tool="bash">echo hello</exec>
<pong/>"#;
        let result = parse(response);
        assert!(result.has_pong);
        assert_eq!(result.actions.len(), 1);
    }

    #[test]
    fn test_parse_self_write() {
        let response = r#"<exec tool="self">write
## My Notes
- Item 1
- Item 2</exec>"#;

        let result = parse(response);
        assert_eq!(result.actions.len(), 1);
        if let Action::Exec { tool, content, .. } = &result.actions[0] {
            assert_eq!(tool, "self");
            assert!(content.starts_with("write\n"));
            assert!(content.contains("## My Notes"));
        } else {
            panic!("expected Exec action");
        }
    }

    #[test]
    fn test_parse_channel_join() {
        let response = r##"<exec tool="channel">join #project-auth</exec>"##;
        let result = parse(response);
        assert_eq!(result.actions.len(), 1);
        assert!(matches!(
            &result.actions[0],
            Action::Exec { tool, content, .. } if tool == "channel" && content == "join #project-auth"
        ));
    }

    #[test]
    fn test_parse_monk_recruit() {
        let response = r#"<exec tool="monk">recruit name=brother-thomas model=sonnet</exec>"#;
        let result = parse(response);
        assert_eq!(result.actions.len(), 1);
        assert!(matches!(
            &result.actions[0],
            Action::Exec { tool, content, .. } if tool == "monk" && content.contains("recruit")
        ));
    }

    #[test]
    fn test_parse_real_world_response() {
        let response = r##"I should check the logs and let the team know what I find.

First, let me read the error log to understand what's happening.

<exec tool="read">/var/log/app/error.log</exec>
<exec tool="bash">tail -20 /var/log/app/access.log</exec>
<say channel="#general">Looking into the recent errors. Will report back shortly.</say>

I'll also make a note to follow up on this.

<exec tool="self">write
## Current Task
Investigating error logs for the team.
Started: 2026-01-28 19:00</exec>"##;

        let result = parse(response);
        assert_eq!(result.actions.len(), 4);
        assert!(!result.has_pong);

        // Verify each action type is present
        let tools: Vec<&str> = result.actions.iter().filter_map(|a| {
            if let Action::Exec { tool, .. } = a { Some(tool.as_str()) } else { None }
        }).collect();

        assert!(tools.contains(&"read"));
        assert!(tools.contains(&"bash"));
        assert!(tools.contains(&"self"));

        let has_say = result.actions.iter().any(|a| matches!(a, Action::Say { .. }));
        assert!(has_say);
    }

    #[test]
    fn test_parse_malformed_ignored() {
        // Malformed tags should not crash the parser
        let response = r##"<exec tool="bash">echo hello
<say channel="#general">broken tag
<exec tool="read">valid</exec>"##;

        let result = parse(response);
        // Parser will find the first exec with bash and consume until </exec>
        // This is expected greedy behavior - the bash exec gets all content
        // The important thing is it doesn't crash
        assert!(!result.actions.is_empty());
    }

    #[test]
    fn test_parse_unclosed_tag_skipped() {
        // Completely unclosed tags are skipped
        let response = r##"<exec tool="bash">echo hello
Some text
<exec tool="read">valid</exec>"##;

        let result = parse(response);
        // bash exec consumes until first </exec>
        // This behavior is acceptable - greedy matching
        assert!(!result.actions.is_empty());
    }
}
