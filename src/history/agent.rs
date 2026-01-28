use std::sync::Arc;
use crate::bus::{Message, MessageOp};
use crate::agent::{Agent, AgentContext};
use super::Store;

pub struct HistoryAgent {
    store: Arc<Store>,
}

impl HistoryAgent {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }
}

impl Agent for HistoryAgent {
    fn name(&self) -> &str {
        "_history"
    }

    fn channels(&self) -> Vec<&str> {
        vec!["#general"]
    }

    async fn on_message(&self, _ctx: &AgentContext, msg: Message) {
        let (content, message_type) = match msg.op {
            MessageOp::Chat => (msg.text().map(|s| s.to_string()), "chat"),
            MessageOp::Ok => (msg.text().map(|s| s.to_string()), "ok"),
            MessageOp::Item => (msg.text().map(|s| s.to_string()), "item"),
            MessageOp::Exec => {
                if let crate::bus::MessageData::Exec { tool, args } = &msg.data {
                    (Some(format!("{}({})", tool, args)), "exec")
                } else {
                    (None, "exec")
                }
            }
            MessageOp::Error => {
                if let crate::bus::MessageData::Error { code, message } = &msg.data {
                    (Some(format!("error [{}]: {}", code, message)), "error")
                } else {
                    (None, "error")
                }
            }
            _ => (None, "other"),
        };

        if let Some(content) = content {
            if let Err(e) = self.store.insert(&msg.channel, &msg.sender, &content, message_type) {
                tracing::error!(?e, "failed to store message");
            }
        }
    }
}
