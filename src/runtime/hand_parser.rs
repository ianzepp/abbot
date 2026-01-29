#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EchoMode {
    None,
    Head,
    Tail,
    Full,
    Summary,
}

impl EchoMode {
    pub fn from_attr(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "head" => EchoMode::Head,
            "tail" => EchoMode::Tail,
            "full" => EchoMode::Full,
            "summary" => EchoMode::Summary,
            "none" | "" => EchoMode::None,
            _ => EchoMode::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecAction {
    pub tool: String,
    pub reason: Option<String>,
    pub destructive: bool,
    pub content: String,
    pub echo: EchoMode,
    pub head: Option<usize>,
    pub tail: Option<usize>,
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
    parsed.execs = parse_exec_tags(response);
    parsed.result = parse_result_tag(response);
    parsed
}

fn parse_exec_tags(text: &str) -> Vec<ExecAction> {
    let mut actions = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("<exec ") {
        let tag_start = &remaining[start..];
        let Some(bracket_end) = tag_start.find('>') else {
            break;
        };
        let opening_tag = &tag_start[..bracket_end];

        let tool = extract_attr(opening_tag, "tool").unwrap_or_default();
        if tool.is_empty() {
            remaining = &remaining[start + 5..];
            continue;
        }

        let reason = extract_attr(opening_tag, "reason");
        let destructive = extract_attr(opening_tag, "destructive")
            .map(|v| v == "true")
            .unwrap_or(false);

        let echo = EchoMode::from_attr(extract_attr(opening_tag, "echo").as_deref().unwrap_or(""));
        let head = extract_attr(opening_tag, "head").and_then(|v| v.parse::<usize>().ok());
        let tail = extract_attr(opening_tag, "tail").and_then(|v| v.parse::<usize>().ok());

        let content_start = &tag_start[bracket_end + 1..];
        let Some(close_tag) = content_start.find("</exec>") else {
            break;
        };
        let content = content_start[..close_tag].to_string();

        actions.push(ExecAction {
            tool,
            reason,
            destructive,
            content,
            echo,
            head,
            tail,
        });

        let total_consumed = start + bracket_end + 1 + close_tag + 7;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    actions
}

fn parse_result_tag(text: &str) -> Option<ResultAction> {
    // Take the last <result ...>...</result> in the response (if any).
    let mut remaining = text;
    let mut last = None::<ResultAction>;

    while let Some(start) = remaining.find("<result ") {
        let tag_start = &remaining[start..];
        let Some(bracket_end) = tag_start.find('>') else {
            break;
        };
        let opening_tag = &tag_start[..bracket_end];
        let ok = extract_attr(opening_tag, "ok").map(|v| v == "true").unwrap_or(false);

        let content_start = &tag_start[bracket_end + 1..];
        let Some(close_tag) = content_start.find("</result>") else {
            break;
        };
        let text = content_start[..close_tag].to_string();
        last = Some(ResultAction { ok, text });

        let total_consumed = start + bracket_end + 1 + close_tag + 9;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    last
}

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
    fn parses_exec_and_result() {
        let r = r#"
thought
<exec tool="bash" reason="list" echo="head" head="2">ls</exec>
<result ok="true">done</result>
"#;
        let parsed = parse_hand_response(r);
        assert_eq!(parsed.execs.len(), 1);
        assert_eq!(parsed.execs[0].tool, "bash");
        assert_eq!(parsed.execs[0].echo, EchoMode::Head);
        assert_eq!(parsed.execs[0].head, Some(2));
        assert_eq!(parsed.result.as_ref().unwrap().ok, true);
    }

    #[test]
    fn takes_last_result() {
        let r = r#"<result ok="false">no</result><result ok="true">yes</result>"#;
        let parsed = parse_hand_response(r);
        assert_eq!(parsed.result.unwrap().text, "yes");
    }
}

