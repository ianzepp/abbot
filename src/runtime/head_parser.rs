use super::parser::{parse_blocks, parse_quoted};

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
        chats: parse_blocks(response, "chat")
            .into_iter()
            .map(|b| ChatAction {
                scope: b.header,
                content: b.content,
            })
            .collect(),
        mails: parse_blocks(response, "mail")
            .into_iter()
            .map(|b| MailAction {
                recipient: b.header,
                content: b.content,
            })
            .collect(),
        hands: parse_blocks(response, "hand")
            .into_iter()
            .map(|b| HandAction {
                commands: parse_hand_commands(&b.content),
            })
            .filter(|h| !h.commands.is_empty())
            .collect(),
    }
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
            if let Some(goal) = parse_quoted(rest.trim()) {
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
