use std::sync::Arc;

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope};
use crate::history::Store;
use crate::llm::{ChatMessage, Role};

pub struct MindBundleConfig {
    pub head_id: String,
    pub scopes: Vec<Scope>,
    pub max_messages: usize,
}

impl MindBundleConfig {
    pub fn new(head_id: impl Into<String>, scopes: Vec<Scope>) -> Self {
        Self {
            head_id: head_id.into(),
            scopes,
            max_messages: 50,
        }
    }
}

pub struct MindBundleBuilder {
    store: Arc<Store>,
    system: String,
    grammar: String,
}

impl MindBundleBuilder {
    pub fn new(store: Arc<Store>) -> Self {
        let system = include_str!("mind_system.md");
        let grammar = include_str!("mind_grammar.md");
        Self {
            store,
            system: system.to_string(),
            grammar: grammar.to_string(),
        }
    }

    pub fn build(&self, cfg: &MindBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System message: identity + grammar
        let system_content = format!("{}\n\n{}", self.system, self.grammar);
        messages.push(ChatMessage::new(Role::System, system_content));

        // User message: LTM + recent head activity
        let user_content = self.build_user_context(cfg);
        messages.push(ChatMessage::new(Role::User, user_content));

        messages
    }

    fn build_user_context(&self, cfg: &MindBundleConfig) -> String {
        let mut sections = Vec::new();

        // Current LTM
        let ltm = self
            .store
            .get_head_ltm(&cfg.head_id)
            .unwrap_or_default();

        sections.push(format!(
            "## Current Long-Term Memory\n\n{}",
            if ltm.is_empty() {
                "(empty - no memories yet)".to_string()
            } else {
                ltm
            }
        ));

        // Recent head activity
        let activity = self.gather_recent_activity(cfg);
        sections.push(format!(
            "## Recent Head Activity\n\n{}",
            if activity.is_empty() {
                "(no recent activity)".to_string()
            } else {
                activity
            }
        ));

        sections.join("\n\n")
    }

    fn gather_recent_activity(&self, cfg: &MindBundleConfig) -> String {
        let mut all_messages: Vec<Message> = Vec::new();

        for scope in &cfg.scopes {
            let scope_str = scope.to_string();
            let scope_messages = self
                .store
                .recent(&scope_str, cfg.max_messages)
                .unwrap_or_default();
            all_messages.extend(scope_messages);
        }

        // Sort by timestamp (oldest first)
        all_messages.sort_by_key(|m| m.timestamp);

        // Render each message
        all_messages
            .iter()
            .filter_map(|msg| render_activity_message(msg))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn render_activity_message(msg: &Message) -> Option<String> {
    let origin_label = match msg.origin {
        Origin::Human => "human",
        Origin::Head => "head",
        Origin::Hand => "hand",
        Origin::System => "system",
    };

    match (&msg.op, &msg.data) {
        (MessageOp::Chat, MessageData::Text(t)) => {
            Some(format!("[{} {}] {}", origin_label, msg.sender, t))
        }
        (MessageOp::Task, MessageData::Task(task_msg)) => {
            use crate::bus::TaskMsg;
            match task_msg {
                TaskMsg::Request { goal, .. } => {
                    Some(format!("[{} {}] delegated: {}", origin_label, msg.sender, goal))
                }
                TaskMsg::Result { ok, summary, .. } => {
                    let status = if *ok { "completed" } else { "failed" };
                    Some(format!(
                        "[{} {}] task {}: {}",
                        origin_label, msg.sender, status, summary
                    ))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::bus::{Hub, Origin, Scope, respond};
    use crate::runtime::RuntimeBus;

    #[tokio::test]
    async fn builds_context_with_ltm_and_activity() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        // Set some LTM
        store.set_head_ltm("Monk", "Curious about: Rust patterns.").unwrap();

        let hub = Arc::new(RwLock::new(Hub::new()));
        let bus = RuntimeBus::new(hub, store.clone());
        bus.create_scope(Scope::from("#general")).await;

        // Add some activity
        bus.publish(
            respond::chat("alice", "#general", "Can you help with this?")
                .with_origin(Origin::Human),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        bus.publish(
            respond::chat("Monk", "#general", "Sure, I'll look into it.")
                .with_origin(Origin::Head),
        )
        .await;

        let builder = MindBundleBuilder::new(store);
        let cfg = MindBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);

        // System message
        assert!(matches!(messages[0].role, Role::System));
        assert!(messages[0].content.contains("Mind"));
        assert!(messages[0].content.contains("ltm append"));

        // User message with LTM and activity
        assert!(matches!(messages[1].role, Role::User));
        assert!(messages[1].content.contains("Long-Term Memory"));
        assert!(messages[1].content.contains("Rust patterns"));
        assert!(messages[1].content.contains("Recent Head Activity"));
        assert!(messages[1].content.contains("alice"));
        assert!(messages[1].content.contains("Monk"));
    }

    #[tokio::test]
    async fn handles_empty_ltm() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        let builder = MindBundleBuilder::new(store);
        let cfg = MindBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);
        assert!(messages[1].content.contains("empty - no memories yet"));
    }
}
