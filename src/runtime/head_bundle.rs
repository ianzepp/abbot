use std::sync::Arc;

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope, TaskMsg};
use crate::history::Store;
use crate::llm::{ChatMessage, Role};
use uuid::Uuid;

pub struct HeadBundleConfig {
    pub head_id: String,
    pub scopes: Vec<Scope>,
    pub max_messages_per_scope: usize,
}

impl HeadBundleConfig {
    pub fn new(head_id: impl Into<String>, scopes: Vec<Scope>) -> Self {
        Self {
            head_id: head_id.into(),
            scopes,
            max_messages_per_scope: 100,
        }
    }
}

pub struct HeadBundleBuilder {
    store: Arc<Store>,
    system: String,
    grammar: String,
}

impl HeadBundleBuilder {
    pub fn new(store: Arc<Store>) -> Self {
        let system = include_str!("head_system.md");
        let grammar = include_str!("head_grammar.md");
        Self {
            store,
            system: system.to_string(),
            grammar: grammar.to_string(),
        }
    }

    pub fn build(&self, cfg: &HeadBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System message: identity + grammar + LTM (if any)
        let ltm = self.store.get_head_ltm(&cfg.head_id).unwrap_or_default();

        let system_content = if ltm.is_empty() {
            format!("{}\n\n{}", self.system, self.grammar)
        } else {
            format!(
                "{}\n\n{}\n\n## Long-Term Memory\n\n{}",
                self.system, self.grammar, ltm
            )
        };
        messages.push(ChatMessage::new(Role::System, system_content));

        // Gather and sort all messages from all scopes by timestamp
        let mut all_messages: Vec<Message> = Vec::new();
        for scope in &cfg.scopes {
            let scope_str = scope.to_string();
            let scope_messages = self
                .store
                .recent(&scope_str, cfg.max_messages_per_scope)
                .unwrap_or_default();
            all_messages.extend(scope_messages);
        }

        // Sort by timestamp (oldest first for conversation order)
        all_messages.sort_by_key(|m| m.timestamp);

        // Convert to chat messages with appropriate roles
        for msg in all_messages {
            let role = self.message_role(&msg, &cfg.head_id);
            let is_self = msg.origin == Origin::Head && msg.sender == cfg.head_id;
            let content = render_message(&msg, is_self);

            if !content.is_empty() {
                messages.push(ChatMessage::new(role, content));
            }
        }

        messages
    }

    fn message_role(&self, msg: &Message, head_id: &str) -> Role {
        // Assistant = this head speaking
        // User = everyone else (humans, other heads, system, hands)
        if msg.origin == Origin::Head && msg.sender == head_id {
            Role::Assistant
        } else {
            Role::User
        }
    }
}

fn render_message(msg: &Message, is_self: bool) -> String {
    // Skip prefix for the head's own messages to avoid teaching it to echo "[Abbot]"
    let prefix = if is_self {
        String::new()
    } else if let Some(reply_to) = msg.reply_to {
        format!("[{}↩{}] ", msg.sender, short_uuid(reply_to))
    } else {
        format!("[{}] ", msg.sender)
    };

    match (&msg.op, &msg.data) {
        (MessageOp::Chat, MessageData::Text(t)) => {
            if is_self {
                t.clone()
            } else {
                format!("{}{}", prefix, t)
            }
        }
        (MessageOp::Task, MessageData::Task(task_msg)) => render_task_message(&prefix, task_msg),
        _ => String::new(),
    }
}

fn short_uuid(id: Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn render_task_message(prefix: &str, task: &TaskMsg) -> String {
    match task {
        TaskMsg::Request { task_id, goal, .. } => {
            format!("{}task {} requested: {}", prefix, task_id, goal)
        }
        TaskMsg::Assigned {
            task_id, hand_id, ..
        } => {
            format!("{}task {} assigned to {}", prefix, task_id, hand_id)
        }
        TaskMsg::Progress { task_id, note, .. } => {
            format!("{}task {} progress: {}", prefix, task_id, note)
        }
        TaskMsg::Echo {
            task_id,
            tool,
            content,
            ..
        } => {
            format!("{}task {} echo from {}: {}", prefix, task_id, tool, content)
        }
        TaskMsg::Result {
            task_id,
            ok,
            summary,
            ..
        } => {
            let status = if *ok { "completed" } else { "failed" };
            format!("{}task {} {}: {}", prefix, task_id, status, summary)
        }
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
    async fn builds_conversation_with_roles() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        let hub = Arc::new(RwLock::new(Hub::new()));
        let bus = RuntimeBus::new(hub, store.clone());
        bus.create_scope(Scope::from("#general")).await;

        // Human says something
        bus.publish(respond::chat("alice", "#general", "hello monk").with_origin(Origin::Human))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Head (Monk) responds
        bus.publish(respond::chat("Monk", "#general", "hello alice").with_origin(Origin::Head))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Human asks question
        bus.publish(respond::chat("alice", "#general", "can you help?").with_origin(Origin::Human))
            .await;

        let builder = HeadBundleBuilder::new(store);
        let cfg = HeadBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        // System + 3 conversation messages
        assert_eq!(messages.len(), 4);

        assert!(matches!(messages[0].role, Role::System));
        assert!(messages[0].content.as_deref().unwrap_or("").contains("Head"));

        // Human message -> User role
        assert!(matches!(messages[1].role, Role::User));
        assert!(messages[1].content.as_deref().unwrap_or("").contains("alice"));
        assert!(messages[1].content.as_deref().unwrap_or("").contains("hello monk"));

        // Head message -> Assistant role
        assert!(matches!(messages[2].role, Role::Assistant));
        assert!(messages[2].content.as_deref().unwrap_or("").contains("Monk"));
        assert!(messages[2].content.as_deref().unwrap_or("").contains("hello alice"));

        // Human message -> User role
        assert!(matches!(messages[3].role, Role::User));
        assert!(messages[3].content.as_deref().unwrap_or("").contains("can you help"));
    }

    #[tokio::test]
    async fn includes_task_messages() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        let hub = Arc::new(RwLock::new(Hub::new()));
        let bus = RuntimeBus::new(hub, store.clone());
        bus.create_scope(Scope::from("#general")).await;

        bus.publish(
            respond::task_result("hand-1", "#general", "t-1", "hand-1", true, "done")
                .with_origin(Origin::Hand),
        )
        .await;

        let builder = HeadBundleBuilder::new(store);
        let cfg = HeadBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[1].role, Role::User));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("task t-1 completed"));
    }
}
