use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatAction {
    pub scope: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailAction {
    pub recipient: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAction {
    pub id: String,
    pub goal: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedHeadResponse {
    pub chats: Vec<ChatAction>,
    pub mails: Vec<MailAction>,
    pub tasks: Vec<TaskAction>,
}

impl ParsedHeadResponse {
    pub fn is_empty(&self) -> bool {
        self.chats.is_empty() && self.mails.is_empty() && self.tasks.is_empty()
    }
}

pub fn parse_head_response(response: &str) -> ParsedHeadResponse {
    ParsedHeadResponse {
        chats: parse_chat_blocks(response),
        mails: parse_mail_blocks(response),
        tasks: parse_task_blocks(response),
    }
}

fn parse_chat_blocks(text: &str) -> Vec<ChatAction> {
    let mut actions = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("--- chat ") {
        let header_start = start + 9;
        let after_marker = &remaining[header_start..];

        let Some(header_end) = after_marker.find(" ---") else {
            break;
        };
        let header = after_marker[..header_end].trim();

        // Header is just the channel: #general
        let scope = header.to_string();

        let content_start = header_end + 4;
        let content_region = &after_marker[content_start..];

        let Some(end_marker) = content_region.find("--- end ---") else {
            break;
        };

        let content = content_region[..end_marker].trim().to_string();

        actions.push(ChatAction { scope, content });

        let total_consumed = header_start + content_start + end_marker + 11;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    actions
}

fn parse_mail_blocks(text: &str) -> Vec<MailAction> {
    let mut actions = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("--- mail ") {
        let header_start = start + 9;
        let after_marker = &remaining[header_start..];

        let Some(header_end) = after_marker.find(" ---") else {
            break;
        };
        let header = after_marker[..header_end].trim();

        // Header is just the recipient: @alice
        let recipient = header.to_string();

        let content_start = header_end + 4;
        let content_region = &after_marker[content_start..];

        let Some(end_marker) = content_region.find("--- end ---") else {
            break;
        };

        let content = content_region[..end_marker].trim().to_string();

        actions.push(MailAction { recipient, content });

        let total_consumed = header_start + content_start + end_marker + 11;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    actions
}

fn parse_task_blocks(text: &str) -> Vec<TaskAction> {
    let mut actions = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("--- task ") {
        let header_start = start + 9;
        let after_marker = &remaining[header_start..];

        let Some(header_end) = after_marker.find(" ---") else {
            break;
        };
        let header = &after_marker[..header_end];

        // Parse header args: id=X goal="Y"
        let args = parse_header_args(header);
        let id = args.get("id").cloned().unwrap_or_default();
        let goal = args.get("goal").cloned().unwrap_or_default();

        let content_start = header_end + 4;
        let content_region = &after_marker[content_start..];

        let Some(end_marker) = content_region.find("--- end ---") else {
            break;
        };

        let content = content_region[..end_marker].trim().to_string();

        actions.push(TaskAction { id, goal, content });

        let total_consumed = header_start + content_start + end_marker + 11;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    actions
}

fn parse_header_args(header: &str) -> HashMap<String, String> {
    let mut args = HashMap::new();
    let mut remaining = header.trim();

    while !remaining.is_empty() {
        // Find key=
        let Some(eq_pos) = remaining.find('=') else {
            break;
        };
        let key = remaining[..eq_pos].trim();
        remaining = &remaining[eq_pos + 1..];

        // Value is either quoted or unquoted
        let value = if remaining.starts_with('"') {
            // Quoted value
            let after_quote = &remaining[1..];
            let Some(end_quote) = after_quote.find('"') else {
                break;
            };
            let val = after_quote[..end_quote].to_string();
            remaining = after_quote[end_quote + 1..].trim_start();
            val
        } else {
            // Unquoted value (until whitespace)
            let end = remaining
                .find(char::is_whitespace)
                .unwrap_or(remaining.len());
            let val = remaining[..end].to_string();
            remaining = remaining[end..].trim_start();
            val
        };

        args.insert(key.to_string(), value);
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chat_block() {
        let r = r#"
thinking here

--- chat #general ---
Hello everyone!
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.chats.len(), 1);
        assert_eq!(parsed.chats[0].scope, "#general");
        assert_eq!(parsed.chats[0].content, "Hello everyone!");
    }

    #[test]
    fn parses_mail_block() {
        let r = r#"
--- mail @alice ---
Here's the info you requested.
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.mails.len(), 1);
        assert_eq!(parsed.mails[0].recipient, "@alice");
        assert_eq!(parsed.mails[0].content, "Here's the info you requested.");
    }

    #[test]
    fn parses_task_block() {
        let r = r#"
--- task id=find-config goal="find where Config is defined" ---
Search the src directory for the Config struct.
Report the file path and line number.
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.tasks.len(), 1);
        assert_eq!(parsed.tasks[0].id, "find-config");
        assert_eq!(parsed.tasks[0].goal, "find where Config is defined");
        assert!(parsed.tasks[0].content.contains("Search the src directory"));
    }

    #[test]
    fn parses_multiple_actions() {
        let r = r#"
Let me help with that.

--- chat #general ---
I'll look into it.
--- end ---

--- task id=investigate goal="check the logs" ---
Look for errors in the log files.
--- end ---

--- chat #general ---
Task created.
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.chats.len(), 2);
        assert_eq!(parsed.tasks.len(), 1);
        assert_eq!(parsed.chats[0].content, "I'll look into it.");
        assert_eq!(parsed.chats[1].content, "Task created.");
    }

    #[test]
    fn parses_task_with_unquoted_args() {
        let r = r#"
--- task id=simple-task goal=doit ---
do the thing
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.tasks[0].id, "simple-task");
        assert_eq!(parsed.tasks[0].goal, "doit");
    }

    #[test]
    fn empty_response() {
        let r = "just some thinking, no actions";
        let parsed = parse_head_response(r);
        assert!(parsed.is_empty());
    }
}
