use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecAction {
    pub tool: String,
    pub args: HashMap<String, String>,
    pub content: String,
}

impl ExecAction {
    pub fn get_arg(&self, key: &str) -> Option<&str> {
        self.args.get(key).map(|s| s.as_str())
    }

    pub fn get_arg_usize(&self, key: &str) -> Option<usize> {
        self.args.get(key).and_then(|s| s.parse().ok())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultAction {
    pub ok: bool,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedHandResponse {
    pub execs: Vec<ExecAction>,
    pub result: Option<ResultAction>,
}

impl ParsedHandResponse {
    pub fn is_empty(&self) -> bool {
        self.execs.is_empty() && self.result.is_none()
    }
}

pub fn parse_hand_response(response: &str) -> ParsedHandResponse {
    let mut parsed = ParsedHandResponse::default();
    parsed.execs = parse_exec_blocks(response);
    parsed.result = parse_result_block(response);
    parsed
}

fn parse_exec_blocks(text: &str) -> Vec<ExecAction> {
    let mut actions = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("--- exec ") {
        let header_start = start + 9; // skip "--- exec "
        let after_marker = &remaining[header_start..];

        // Find end of header line (the closing " ---")
        let Some(header_end) = after_marker.find(" ---") else {
            break;
        };
        let header = &after_marker[..header_end];

        // Parse header: "TOOL [key=value ...]"
        let mut parts = header.split_whitespace();
        let Some(tool) = parts.next() else {
            remaining = &remaining[start + 9..];
            continue;
        };

        let mut args = HashMap::new();
        for part in parts {
            if let Some((key, value)) = part.split_once('=') {
                args.insert(key.to_string(), value.to_string());
            }
        }

        // Find content between header and "--- end ---"
        let content_start = header_end + 4; // skip " ---"
        let content_region = &after_marker[content_start..];

        let Some(end_marker) = content_region.find("--- end ---") else {
            break;
        };

        let content = content_region[..end_marker].trim().to_string();

        actions.push(ExecAction {
            tool: tool.to_string(),
            args,
            content,
        });

        let total_consumed = header_start + content_start + end_marker + 11;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    actions
}

fn parse_result_block(text: &str) -> Option<ResultAction> {
    // Take the last result block in the response by position
    let mut last: Option<(usize, ResultAction)> = None;

    for marker in ["--- result ok ---", "--- result fail ---"] {
        let ok = marker.contains(" ok ");
        let mut search_start = 0;

        while let Some(rel_start) = text[search_start..].find(marker) {
            let start = search_start + rel_start;
            let content_start = start + marker.len();
            let content_region = &text[content_start..];

            let Some(end_marker) = content_region.find("--- end ---") else {
                break;
            };

            let content = content_region[..end_marker].trim().to_string();
            let action = ResultAction { ok, text: content };

            match &last {
                None => last = Some((start, action)),
                Some((prev_start, _)) if start > *prev_start => last = Some((start, action)),
                _ => {}
            }

            search_start = content_start + end_marker + 11;
            if search_start >= text.len() {
                break;
            }
        }
    }

    last.map(|(_, action)| action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exec_block() {
        let r = r#"
thinking here

--- exec bash ---
rg -n "struct Config" src
--- end ---
"#;
        let parsed = parse_hand_response(r);
        assert_eq!(parsed.execs.len(), 1);
        assert_eq!(parsed.execs[0].tool, "bash");
        assert_eq!(parsed.execs[0].content, "rg -n \"struct Config\" src");
    }

    #[test]
    fn parses_exec_with_args() {
        let r = r#"
--- exec read offset=100 limit=50 ---
src/config.rs
--- end ---
"#;
        let parsed = parse_hand_response(r);
        assert_eq!(parsed.execs.len(), 1);
        assert_eq!(parsed.execs[0].tool, "read");
        assert_eq!(parsed.execs[0].get_arg("offset"), Some("100"));
        assert_eq!(parsed.execs[0].get_arg_usize("limit"), Some(50));
        assert_eq!(parsed.execs[0].content, "src/config.rs");
    }

    #[test]
    fn parses_exec_with_path() {
        let r = r#"
--- exec write path=src/new.rs ---
fn main() {}
--- end ---
"#;
        let parsed = parse_hand_response(r);
        assert_eq!(parsed.execs[0].tool, "write");
        assert_eq!(parsed.execs[0].get_arg("path"), Some("src/new.rs"));
        assert_eq!(parsed.execs[0].content, "fn main() {}");
    }

    #[test]
    fn parses_result_ok() {
        let r = r#"
--- result ok ---
Found the file at src/lib.rs
--- end ---
"#;
        let parsed = parse_hand_response(r);
        assert!(parsed.result.is_some());
        assert!(parsed.result.as_ref().unwrap().ok);
        assert_eq!(
            parsed.result.as_ref().unwrap().text,
            "Found the file at src/lib.rs"
        );
    }

    #[test]
    fn parses_result_fail() {
        let r = r#"
--- result fail ---
Could not locate the struct
--- end ---
"#;
        let parsed = parse_hand_response(r);
        assert!(parsed.result.is_some());
        assert!(!parsed.result.as_ref().unwrap().ok);
    }

    #[test]
    fn takes_last_result() {
        let r = r#"
--- result fail ---
first attempt failed
--- end ---

--- result ok ---
second attempt worked
--- end ---
"#;
        let parsed = parse_hand_response(r);
        let result = parsed.result.unwrap();
        assert!(result.ok);
        assert_eq!(result.text, "second attempt worked");
    }

    #[test]
    fn parses_multiline_content() {
        let r = r#"
--- exec write path=test.txt ---
line 1
line 2
line 3
--- end ---
"#;
        let parsed = parse_hand_response(r);
        assert_eq!(parsed.execs[0].content, "line 1\nline 2\nline 3");
    }
}
