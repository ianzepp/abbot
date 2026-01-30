use std::sync::Arc;

use crate::history::Store;
use crate::llm::{ChatMessage, Role};

pub struct HandBundleConfig {
    pub task_id: String,
    pub goal: String,
    pub input: String,
}

impl HandBundleConfig {
    pub fn new(
        task_id: impl Into<String>,
        goal: impl Into<String>,
        input: impl Into<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            goal: goal.into(),
            input: input.into(),
        }
    }
}

pub struct HandBundleBuilder {
    store: Arc<Store>,
    system: String,
    grammar: String,
}

impl HandBundleBuilder {
    pub fn new(store: Arc<Store>) -> Self {
        let system = include_str!("hand_system.md");
        let grammar = include_str!("hand_grammar.md");
        Self {
            store,
            system: system.to_string(),
            grammar: grammar.to_string(),
        }
    }

    pub fn build(&self, cfg: &HandBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System message: playbook + grammar
        let system_content = format!("{}\n\n{}", self.system, self.grammar);
        messages.push(ChatMessage::new(Role::System, system_content));

        // Initial user message: task goal and input
        let initial_prompt = build_initial_prompt(&cfg.goal, &cfg.input);
        messages.push(ChatMessage::new(Role::User, initial_prompt));

        // Load conversation history from DB
        let history = self.store.get_hand_execs(&cfg.task_id).unwrap_or_default();

        for record in history {
            // Add assistant turn (hand's thought/response)
            if !record.hand_thought.is_empty() {
                messages.push(ChatMessage::new(
                    Role::Assistant,
                    record.hand_thought.clone(),
                ));
            }

            // Add user turn (tool result)
            let tool_result = if record.success {
                format!("[Tool {} completed]\n{}", record.tool, record.output)
            } else {
                format!("[Tool {} failed]\n{}", record.tool, record.output)
            };
            messages.push(ChatMessage::new(Role::User, tool_result));
        }

        messages
    }
}

fn build_initial_prompt(goal: &str, input: &str) -> String {
    let mut out = String::new();
    out.push_str("TASK\n");
    out.push_str("goal: ");
    out.push_str(goal.trim());
    out.push('\n');
    if !input.trim().is_empty() {
        out.push_str("input:\n");
        out.push_str(input.trim());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_initial_messages() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let builder = HandBundleBuilder::new(store);

        let cfg = HandBundleConfig::new("t-1", "list files", "");
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].role, Role::System));
        assert!(messages[0]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("You are a hand"));
        assert!(messages[0]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("Hand Tool Calling"));
        assert!(matches!(messages[1].role, Role::User));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("goal: list files"));
    }

    #[test]
    fn builds_conversation_from_history() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        // Log some hand execs
        store
            .log_hand_exec(
                "t-2",
                "hand-1",
                0,
                "bash",
                "ls",
                "file1\nfile2",
                true,
                10,
                "<exec tool=\"bash\">ls</exec>",
            )
            .unwrap();
        store
            .log_hand_exec(
                "t-2",
                "hand-1",
                1,
                "read",
                "file1",
                "contents",
                true,
                5,
                "<exec tool=\"read\">file1</exec>",
            )
            .unwrap();

        let builder = HandBundleBuilder::new(store);
        let cfg = HandBundleConfig::new("t-2", "read files", "");
        let messages = builder.build(&cfg);

        // System + Initial + 2*(Assistant + User)
        assert_eq!(messages.len(), 6);

        assert!(matches!(messages[0].role, Role::System));
        assert!(matches!(messages[1].role, Role::User));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("goal: read files"));

        assert!(matches!(messages[2].role, Role::Assistant));
        assert!(messages[2]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("<exec tool=\"bash\">ls</exec>"));

        assert!(matches!(messages[3].role, Role::User));
        assert!(messages[3]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("[Tool bash completed]"));
        assert!(messages[3]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("file1\nfile2"));

        assert!(matches!(messages[4].role, Role::Assistant));
        assert!(messages[4]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("<exec tool=\"read\">file1</exec>"));

        assert!(matches!(messages[5].role, Role::User));
        assert!(messages[5]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("[Tool read completed]"));
        assert!(messages[5]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("contents"));
    }
}
