use super::parser::{extract_plain_text, parse_fenced_blocks, parse_quoted};

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

pub fn parse_head_response(response: &str, default_scope: &str) -> ParsedHeadResponse {
    let blocks = parse_fenced_blocks(response);
    let plain_text = extract_plain_text(response);

    let mut chats: Vec<ChatAction> = Vec::new();
    let mut mails: Vec<MailAction> = Vec::new();
    let mut hands: Vec<HandAction> = Vec::new();

    for block in blocks {
        match block.tag.as_str() {
            "chat" => {
                let scope = if block.header.is_empty() {
                    default_scope.to_string()
                } else {
                    block.header
                };
                if !block.content.is_empty() {
                    chats.push(ChatAction {
                        scope,
                        content: block.content,
                    });
                }
            }
            "mail" => {
                if !block.header.is_empty() && !block.content.is_empty() {
                    mails.push(MailAction {
                        recipient: block.header,
                        content: block.content,
                    });
                }
            }
            "hand" => {
                let commands = parse_hand_commands(&block.content);
                if !commands.is_empty() {
                    hands.push(HandAction { commands });
                }
            }
            _ => {}
        }
    }

    if !plain_text.is_empty() {
        chats.insert(
            0,
            ChatAction {
                scope: default_scope.to_string(),
                content: plain_text,
            },
        );
    }

    ParsedHeadResponse {
        chats,
        mails,
        hands,
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
    fn parses_plain_text_as_default_chat() {
        let r = "Hello everyone!";
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.chats.len(), 1);
        assert_eq!(parsed.chats[0].scope, "#general");
        assert_eq!(parsed.chats[0].content, "Hello everyone!");
    }

    #[test]
    fn parses_chat_block_to_other_scope() {
        let r = r#"
```chat #dev
Build completed.
```
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.chats.len(), 1);
        assert_eq!(parsed.chats[0].scope, "#dev");
        assert_eq!(parsed.chats[0].content, "Build completed.");
    }

    #[test]
    fn parses_mail_block() {
        let r = r#"
```mail @alice
Here's the info you requested.
```
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.mails.len(), 1);
        assert_eq!(parsed.mails[0].recipient, "@alice");
        assert_eq!(parsed.mails[0].content, "Here's the info you requested.");
    }

    #[test]
    fn parses_mixed_plain_and_blocks() {
        let r = r#"
I'll look into that for you.

```hand
goal "check the logs"
```

Let me know if you need anything else.
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.chats.len(), 1);
        assert_eq!(
            parsed.chats[0].content,
            "I'll look into that for you.\nLet me know if you need anything else."
        );
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(
            parsed.hands[0].commands,
            vec![HandCommand::Goal("check the logs".to_string())]
        );
    }

    #[test]
    fn empty_response() {
        let r = "";
        let parsed = parse_head_response(r, "#general");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parses_hand_list() {
        let r = r#"
```hand
list
```
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(parsed.hands[0].commands, vec![HandCommand::List]);
    }

    #[test]
    fn parses_hand_read() {
        let r = r#"
```hand
read 0
read 1
```
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(
            parsed.hands[0].commands,
            vec![HandCommand::Read(0), HandCommand::Read(1)]
        );
    }

    #[test]
    fn parses_hand_clear() {
        let r = r#"
```hand
clear 0
```
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(parsed.hands[0].commands, vec![HandCommand::Clear(0)]);
    }

    #[test]
    fn parses_hand_mixed_commands() {
        let r = r#"
```hand
list
read 0
clear 1
```
"#;
        let parsed = parse_head_response(r, "#general");
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
```hand
goal "find all rust files"
```
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.hands.len(), 1);
        assert_eq!(
            parsed.hands[0].commands,
            vec![HandCommand::Goal("find all rust files".to_string())]
        );
    }

    #[test]
    fn parses_multiple_goals() {
        let r = r#"
```hand
goal "count rust files"
goal "count markdown files"
goal "list src directory"
```
"#;
        let parsed = parse_head_response(r, "#general");
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

    #[test]
    fn plain_text_comes_first() {
        let r = r#"
```chat #dev
Dev message
```

Plain text here.
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.chats.len(), 2);
        assert_eq!(parsed.chats[0].scope, "#general");
        assert_eq!(parsed.chats[0].content, "Plain text here.");
        assert_eq!(parsed.chats[1].scope, "#dev");
        assert_eq!(parsed.chats[1].content, "Dev message");
    }
}
