use std::collections::HashMap;

use super::parser::parse_blocks;

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
    ParsedHandResponse {
        execs: parse_exec_blocks(response),
        result: parse_result_block(response),
    }
}

fn parse_exec_blocks(text: &str) -> Vec<ExecAction> {
    parse_blocks(text, "exec")
        .into_iter()
        .map(|b| {
            let mut parts = b.header.split_whitespace();
            let tool = parts.next().unwrap_or("").to_string();

            let mut args = HashMap::new();
            for part in parts {
                if let Some((key, value)) = part.split_once('=') {
                    args.insert(key.to_string(), value.to_string());
                }
            }

            ExecAction {
                tool,
                args,
                content: b.content,
            }
        })
        .collect()
}

fn parse_result_block(text: &str) -> Option<ResultAction> {
    let ok_blocks = parse_blocks(text, "result ok");
    let fail_blocks = parse_blocks(text, "result fail");

    let last_ok = ok_blocks
        .last()
        .and_then(|b| text.find("--- result ok ---").map(|pos| (pos, &b.content)));
    let last_fail = fail_blocks.last().and_then(|b| {
        text.find("--- result fail ---")
            .map(|pos| (pos, &b.content))
    });

    match (last_ok, last_fail) {
        (Some((ok_pos, ok_content)), Some((fail_pos, fail_content))) => {
            if ok_pos > fail_pos {
                Some(ResultAction {
                    ok: true,
                    text: ok_content.clone(),
                })
            } else {
                Some(ResultAction {
                    ok: false,
                    text: fail_content.clone(),
                })
            }
        }
        (Some((_, content)), None) => Some(ResultAction {
            ok: true,
            text: content.clone(),
        }),
        (None, Some((_, content))) => Some(ResultAction {
            ok: false,
            text: content.clone(),
        }),
        (None, None) => None,
    }
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
