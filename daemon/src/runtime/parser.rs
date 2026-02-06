/// A parsed fenced code block.
/// Format: ```tag header\ncontent\n```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub tag: String,
    pub header: String,
    pub content: String,
}

/// Parse all fenced code blocks from text.
/// Returns blocks in order of appearance.
/// Format: ```tag header\ncontent\n``` or ```tag\ncontent\n```
pub fn parse_fenced_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("```") {
        let after_open = &remaining[start + 3..];

        let Some(line_end) = after_open.find('\n') else {
            break;
        };

        let tag_line = after_open[..line_end].trim();
        let (tag, header) = parse_tag_line(tag_line);

        let content_start = &after_open[line_end + 1..];

        let Some(close) = content_start.find("```") else {
            break;
        };

        let content = content_start[..close].trim().to_string();

        blocks.push(Block {
            tag,
            header,
            content,
        });

        let consumed = start + 3 + line_end + 1 + close + 3;
        if consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[consumed..];
    }

    blocks
}

fn parse_tag_line(line: &str) -> (String, String) {
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    match parts.as_slice() {
        [tag] => (tag.to_string(), String::new()),
        [tag, header] => (tag.to_string(), header.to_string()),
        _ => (String::new(), String::new()),
    }
}

/// Extract plain text outside fenced blocks.
/// Returns concatenated text from outside all ``` blocks.
pub fn extract_plain_text(text: &str) -> String {
    let mut plain = String::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("```") {
        plain.push_str(&remaining[..start]);

        let after_open = &remaining[start + 3..];
        let Some(line_end) = after_open.find('\n') else {
            break;
        };
        let content_start = &after_open[line_end + 1..];
        let Some(close) = content_start.find("```") else {
            break;
        };

        let consumed = start + 3 + line_end + 1 + close + 3;
        if consumed >= remaining.len() {
            remaining = "";
            break;
        }
        remaining = &remaining[consumed..];
    }

    plain.push_str(remaining);

    // Trim each line but preserve blank lines (paragraph breaks)
    let lines: Vec<&str> = plain.lines().map(|l| l.trim()).collect();

    // Collapse consecutive blank lines to single blank line, trim leading/trailing
    let mut result = Vec::new();
    let mut prev_blank = true; // Start true to skip leading blanks

    for line in lines {
        if line.is_empty() {
            if !prev_blank {
                result.push("");
                prev_blank = true;
            }
        } else {
            result.push(line);
            prev_blank = false;
        }
    }

    // Remove trailing blank line if present
    if result.last() == Some(&"") {
        result.pop();
    }

    result.join("\n")
}

/// Parse a quoted string like `"hello world"` and return the inner content.
pub fn parse_quoted(s: &str) -> Option<String> {
    if s.starts_with('"')
        && s.len() > 1
        && let Some(end) = s[1..].find('"')
    {
        return Some(s[1..end + 1].to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_block() {
        let text = r#"
some text

```chat #general
Hello everyone!
```

more text
"#;
        let blocks = parse_fenced_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].tag, "chat");
        assert_eq!(blocks[0].header, "#general");
        assert_eq!(blocks[0].content, "Hello everyone!");
    }

    #[test]
    fn parses_multiple_blocks() {
        let text = r#"
```hand
goal "test 1"
```

```hand
goal "test 2"
```
"#;
        let blocks = parse_fenced_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].content, "goal \"test 1\"");
        assert_eq!(blocks[1].content, "goal \"test 2\"");
    }

    #[test]
    fn parses_empty_header() {
        let text = r#"
```hand
list
goal "test"
```
"#;
        let blocks = parse_fenced_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].tag, "hand");
        assert_eq!(blocks[0].header, "");
        assert_eq!(blocks[0].content, "list\ngoal \"test\"");
    }

    #[test]
    fn parses_multiline_content() {
        let text = r#"
```chat #dev
line 1
line 2
line 3
```
"#;
        let blocks = parse_fenced_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "line 1\nline 2\nline 3");
    }

    #[test]
    fn extracts_plain_text() {
        let text = r#"
Hello there!

```hand
goal "test"
```

How are you?

```chat #dev
internal
```

Goodbye!
"#;
        let plain = extract_plain_text(text);
        assert_eq!(plain, "Hello there!\n\nHow are you?\n\nGoodbye!");
    }

    #[test]
    fn extracts_plain_text_no_blocks() {
        let text = "Hello world!\nHow are you?";
        let plain = extract_plain_text(text);
        assert_eq!(plain, "Hello world!\nHow are you?");
    }

    #[test]
    fn parses_quoted_string() {
        assert_eq!(
            parse_quoted("\"hello world\""),
            Some("hello world".to_string())
        );
        assert_eq!(parse_quoted("\"test\""), Some("test".to_string()));
        assert_eq!(parse_quoted("no quotes"), None);
        assert_eq!(parse_quoted("\"unclosed"), None);
    }
}
