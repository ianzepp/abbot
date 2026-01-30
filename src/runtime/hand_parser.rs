use std::collections::HashMap;

use super::parser::parse_fenced_blocks;

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
    let blocks = parse_fenced_blocks(response);

    let mut execs = Vec::new();
    let mut result = None;

    for block in blocks {
        if block.tag == "exec" {
            let mut parts = block.header.split_whitespace();
            let tool = parts.next().unwrap_or("").to_string();

            let mut args = HashMap::new();
            for part in parts {
                if let Some((key, value)) = part.split_once('=') {
                    args.insert(key.to_string(), value.to_string());
                }
            }

            execs.push(ExecAction {
                tool,
                args,
                content: block.content,
            });
        } else if block.tag == "result" {
            let ok = block.header == "ok";
            result = Some(ResultAction {
                ok,
                text: block.content,
            });
        }
    }

    ParsedHandResponse { execs, result }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exec_block() {
        let r = r#"
thinking here

```exec bash
rg -n "struct Config" src
```
"#;
        let parsed = parse_hand_response(r);
        assert_eq!(parsed.execs.len(), 1);
        assert_eq!(parsed.execs[0].tool, "bash");
        assert_eq!(parsed.execs[0].content, "rg -n \"struct Config\" src");
    }

    #[test]
    fn parses_exec_with_args() {
        let r = r#"
```exec read offset=100 limit=50
src/config.rs
```
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
```exec write path=src/new.rs
fn main() {}
```
"#;
        let parsed = parse_hand_response(r);
        assert_eq!(parsed.execs[0].tool, "write");
        assert_eq!(parsed.execs[0].get_arg("path"), Some("src/new.rs"));
        assert_eq!(parsed.execs[0].content, "fn main() {}");
    }

    #[test]
    fn parses_result_ok() {
        let r = r#"
```result ok
Found the file at src/lib.rs
```
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
```result fail
Could not locate the struct
```
"#;
        let parsed = parse_hand_response(r);
        assert!(parsed.result.is_some());
        assert!(!parsed.result.as_ref().unwrap().ok);
    }

    #[test]
    fn takes_last_result() {
        let r = r#"
```result fail
first attempt failed
```

```result ok
second attempt worked
```
"#;
        let parsed = parse_hand_response(r);
        let result = parsed.result.unwrap();
        assert!(result.ok);
        assert_eq!(result.text, "second attempt worked");
    }

    #[test]
    fn parses_multiline_content() {
        let r = r#"
```exec write path=test.txt
line 1
line 2
line 3
```
"#;
        let parsed = parse_hand_response(r);
        assert_eq!(parsed.execs[0].content, "line 1\nline 2\nline 3");
    }
}
