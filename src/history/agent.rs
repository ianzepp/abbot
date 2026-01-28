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
}

impl Agent for HistoryAgent {
    fn name(&self) -> &str {
        "_history"
    }

    fn channels(&self) -> Vec<&str> {
        vec!["#general"]
    }

    async fn on_message(&self, _ctx: &AgentContext, msg: Message) {
        if msg.op == MessageOp::Ping {
            return;
        }
        if let Err(e) = self.store.insert(&msg) {
            tracing::error!(?e, "failed to store message");
        }
    }
}
