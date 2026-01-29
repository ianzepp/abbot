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
pub struct HandAction {
    pub commands: Vec<HandCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandCommand {
    List,
    Goal(String),
    Read(usize),
    Clear(usize),
}

#[derive(Debug, Clone, Default)]
pub struct ParsedHeadResponse {
    pub chats: Vec<ChatAction>,
    pub mails: Vec<MailAction>,
    pub hands: Vec<HandAction>,
}

impl ParsedHeadResponse {
    pub fn is_empty(&self) -> bool {
        self.chats.is_empty() && self.mails.is_empty() && self.hands.is_empty()
    }
}

pub fn parse_head_response(response: &str) -> ParsedHeadResponse {
    ParsedHeadResponse {
        chats: parse_chat_blocks(response),
        mails: parse_mail_blocks(response),
        hands: parse_hand_blocks(response),
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

fn parse_hand_blocks(text: &str) -> Vec<HandAction> {
    let mut actions = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("--- hand ---") {
        let content_start = start + 12;
        let after_marker = &remaining[content_start..];

        let Some(end_marker) = after_marker.find("--- end ---") else {
            break;
        };

        let content = after_marker[..end_marker].trim();
        let commands = parse_hand_commands(content);

        if !commands.is_empty() {
            actions.push(HandAction { commands });
        }

        let total_consumed = content_start + end_marker + 11;
        if total_consumed >= remaining.len() {
            break;
        }
        remaining = &remaining[total_consumed..];
    }

    actions
}

fn parse_hand_commands(content: &str) -> Vec<HandCommand> {
    let mut commands = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line == "list" {
            commands.push(HandCommand::List);
        } else if let Some(rest) = line.strip_prefix("goal ") {
            if let Some(goal) = parse_quoted_string(rest.trim()) {
                commands.push(HandCommand::Goal(goal));
            }
        } else if let Some(rest) = line.strip_prefix("read ") {
            if let Ok(n) = rest.trim().parse::<usize>() {
                commands.push(HandCommand::Read(n));
            }
        } else if let Some(rest) = line.strip_prefix("clear ") {
            if let Ok(n) = rest.trim().parse::<usize>() {
                commands.push(HandCommand::Clear(n));
            }
        }
    }

    commands
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
    fn parses_multiple_actions() {
        let r = r#"
Let me help with that.

--- chat #general ---
I'll look into it.
--- end ---

--- hand ---
goal "check the logs"
--- end ---

--- chat #general ---
Task created.
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.chats.len(), 2);
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(
            parsed.hands[0].commands,
            vec![HandCommand::Goal("check the logs".to_string())]
        );
        assert_eq!(parsed.chats[0].content, "I'll look into it.");
        assert_eq!(parsed.chats[1].content, "Task created.");
    }

    #[test]
    fn empty_response() {
        let r = "just some thinking, no actions";
        let parsed = parse_head_response(r);
        assert!(parsed.is_empty());
    }

    #[test]
    fn parses_hand_list() {
        let r = r#"
--- hand ---
list
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(parsed.hands[0].commands, vec![HandCommand::List]);
    }

    #[test]
    fn parses_hand_read() {
        let r = r#"
--- hand ---
read 0
read 1
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(
            parsed.hands[0].commands,
            vec![HandCommand::Read(0), HandCommand::Read(1)]
        );
    }

    #[test]
    fn parses_hand_clear() {
        let r = r#"
--- hand ---
clear 0
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(parsed.hands[0].commands, vec![HandCommand::Clear(0)]);
    }

    #[test]
    fn parses_hand_mixed_commands() {
        let r = r#"
--- hand ---
list
read 0
clear 1
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(
            parsed.hands[0].commands,
            vec![
                HandCommand::List,
                HandCommand::Read(0),
                HandCommand::Clear(1),
            ]
        );
    }

    #[test]
    fn parses_hand_goal() {
        let r = r#"
--- hand ---
goal "find all rust files"
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(
            parsed.hands[0].commands,
            vec![HandCommand::Goal("find all rust files".to_string())]
        );
    }

    #[test]
    fn parses_multiple_goals() {
        let r = r#"
--- hand ---
goal "count rust files"
goal "count markdown files"
goal "list src directory"
--- end ---
"#;
        let parsed = parse_head_response(r);
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(
            parsed.hands[0].commands,
            vec![
                HandCommand::Goal("count rust files".to_string()),
                HandCommand::Goal("count markdown files".to_string()),
                HandCommand::Goal("list src directory".to_string()),
            ]
        );
    }
}
