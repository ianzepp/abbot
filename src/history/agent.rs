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
        // Only store chat messages and responses with text
        let content = match msg.op {
            MessageOp::Chat | MessageOp::Ok | MessageOp::Item => {
                msg.text().map(|s| s.to_string())
            }
            MessageOp::Error => {
                if let crate::bus::MessageData::Error { code, message } = &msg.data {
                    Some(format!("error [{}]: {}", code, message))
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(content) = content {
            if let Err(e) = self.store.insert(&msg.channel, &msg.sender, &content) {
                tracing::error!(?e, "failed to store message");
            }
        }
    }
}
