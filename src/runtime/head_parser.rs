use super::parser::{extract_plain_text, parse_fenced_blocks};

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
pub struct GoalAction {
    pub goal: String,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedHeadResponse {
    pub chats: Vec<ChatAction>,
    pub mails: Vec<MailAction>,
    pub goals: Vec<GoalAction>,
}

impl ParsedHeadResponse {
    pub fn is_empty(&self) -> bool {
        self.chats.is_empty() && self.mails.is_empty() && self.goals.is_empty()
    }
}

pub fn parse_head_response(response: &str, default_scope: &str) -> ParsedHeadResponse {
    let blocks = parse_fenced_blocks(response);
    let plain_text = extract_plain_text(response);

    let mut chats: Vec<ChatAction> = Vec::new();
    let mut mails: Vec<MailAction> = Vec::new();
    let mut goals: Vec<GoalAction> = Vec::new();

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
            "goal" => {
                let goal_text = block.content.trim();
                if !goal_text.is_empty() {
                    goals.push(GoalAction {
                        goal: goal_text.to_string(),
                    });
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
        goals,
    }
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
    fn parses_goal_block() {
        let r = r#"
```goal
find all rust files in src/
```
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.goals.len(), 1);
        assert_eq!(parsed.goals[0].goal, "find all rust files in src/");
    }

    #[test]
    fn parses_multiple_goal_blocks() {
        let r = r#"
```goal
count rust files
```

```goal
count markdown files
```
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.goals.len(), 2);
        assert_eq!(parsed.goals[0].goal, "count rust files");
        assert_eq!(parsed.goals[1].goal, "count markdown files");
    }

    #[test]
    fn parses_mixed_plain_and_blocks() {
        let r = r#"
I'll look into that for you.

```goal
check the logs
```

Let me know if you need anything else.
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.chats.len(), 1);
        assert_eq!(
            parsed.chats[0].content,
            "I'll look into that for you.\nLet me know if you need anything else."
        );
        assert_eq!(parsed.goals.len(), 1);
        assert_eq!(parsed.goals[0].goal, "check the logs");
    }

    #[test]
    fn empty_response() {
        let r = "";
        let parsed = parse_head_response(r, "#general");
        assert!(parsed.is_empty());
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

    #[test]
    fn multiline_goal() {
        let r = r#"
```goal
Search for all TODO comments in the codebase
and create a summary report
```
"#;
        let parsed = parse_head_response(r, "#general");
        assert_eq!(parsed.goals.len(), 1);
        assert_eq!(
            parsed.goals[0].goal,
            "Search for all TODO comments in the codebase\nand create a summary report"
        );
    }
}
