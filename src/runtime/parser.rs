/// A parsed block from the triple-dash format.
/// Format: `--- TYPE HEADER ---\nCONTENT\n--- end ---`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub block_type: String,
    pub header: String,
    pub content: String,
}

/// Parse all blocks of a given type from text.
/// Returns blocks in order of appearance.
/// Handles both `--- TYPE ---` (no header) and `--- TYPE HEADER ---` formats.
pub fn parse_blocks(text: &str, block_type: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let marker_prefix = format!("--- {}", block_type);
    let mut remaining = text;

    while let Some(start) = remaining.find(&marker_prefix) {
        let after_prefix = &remaining[start + marker_prefix.len()..];

        let (header, content_region) = if after_prefix.starts_with(" ---") {
            (String::new(), &after_prefix[4..])
        } else if after_prefix.starts_with(' ') {
            let after_space = &after_prefix[1..];
            let Some(header_end) = after_space.find(" ---") else {
                break;
            };
            let header = after_space[..header_end].trim().to_string();
            (header, &after_space[header_end + 4..])
        } else {
            remaining = &remaining[start + marker_prefix.len()..];
            continue;
        };

        let Some(end_marker) = content_region.find("--- end ---") else {
            break;
        };

        let content = content_region[..end_marker].trim().to_string();

        blocks.push(Block {
            block_type: block_type.to_string(),
            header,
            content,
        });

        let block_end_in_region = end_marker + 11;
        let region_start_in_remaining = remaining.len() - content_region.len();
        let total_consumed = region_start_in_remaining + block_end_in_region;

        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    blocks
}

/// Parse a quoted string like `"hello world"` and return the inner content.
pub fn parse_quoted(s: &str) -> Option<String> {
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
    fn parses_single_block() {
        let text = r#"
some text

--- chat #general ---
Hello everyone!
--- end ---

more text
"#;
        let blocks = parse_blocks(text, "chat");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_type, "chat");
        assert_eq!(blocks[0].header, "#general");
        assert_eq!(blocks[0].content, "Hello everyone!");
    }

    #[test]
    fn parses_multiple_blocks() {
        let text = r#"
--- exec bash ---
ls -la
--- end ---

--- exec read offset=10 ---
file.txt
--- end ---
"#;
        let blocks = parse_blocks(text, "exec");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].header, "bash");
        assert_eq!(blocks[0].content, "ls -la");
        assert_eq!(blocks[1].header, "read offset=10");
        assert_eq!(blocks[1].content, "file.txt");
    }

    #[test]
    fn parses_empty_header() {
        let text = r#"
--- hand ---
list
goal "test"
--- end ---
"#;
        let blocks = parse_blocks(text, "hand");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].header, "");
        assert_eq!(blocks[0].content, "list\ngoal \"test\"");
    }

    #[test]
    fn ignores_other_block_types() {
        let text = r#"
--- chat #general ---
hello
--- end ---

--- mail @alice ---
hi
--- end ---
"#;
        let chat_blocks = parse_blocks(text, "chat");
        assert_eq!(chat_blocks.len(), 1);
        assert_eq!(chat_blocks[0].header, "#general");

        let mail_blocks = parse_blocks(text, "mail");
        assert_eq!(mail_blocks.len(), 1);
        assert_eq!(mail_blocks[0].header, "@alice");
    }

    #[test]
    fn parses_multiline_content() {
        let text = r#"
--- result ok ---
line 1
line 2
line 3
--- end ---
"#;
        let blocks = parse_blocks(text, "result");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "line 1\nline 2\nline 3");
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
