// Hub is the central pub/sub coordinator for the message bus.
//
// It manages per-scope channels and a global "all" channel for services
// that need to monitor every message (like persistence and audit logging).
// Uses tokio's broadcast channels for efficient multi-producer multi-consumer
// messaging without backpressure concerns.

use super::Scope;
use super::{Channel, Message};
use std::collections::HashMap;
use tokio::sync::broadcast;

// Hub manages all channels and provides both scoped and global subscriptions.
// Messages published to a scope are sent to that scope's subscribers AND
// the global all_tx channel, enabling both targeted and broadcast patterns.
pub struct Hub {
    channels: HashMap<Scope, Channel>,
    all_tx: broadcast::Sender<Message>,
}

impl Hub {
    pub fn new() -> Self {
        let (all_tx, _) = broadcast::channel(1024);
        Self {
            channels: HashMap::new(),
            all_tx,
        }
    }

    pub fn create_scope(&mut self, scope: Scope) -> &Channel {
        self.channels
            .entry(scope.clone())
            .or_insert_with(|| Channel::new(scope.clone()));
        self.channels.get(&scope).unwrap()
    }

    pub fn publish(&mut self, msg: Message) {
        let scope = msg.scope.clone();
        self.create_scope(scope.clone());
        let _ = self.all_tx.send(msg.clone());
        if let Some(ch) = self.channels.get(&scope) {
            ch.publish(msg);
        }
    }

    pub fn subscribe(&self, scope: &Scope) -> Option<broadcast::Receiver<Message>> {
        self.channels.get(scope).map(|ch| ch.subscribe())
    }

    pub fn subscribe_all(&self) -> broadcast::Receiver<Message> {
        self.all_tx.subscribe()
    }

    pub fn scopes(&self) -> Vec<Scope> {
        self.channels.keys().cloned().collect()
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}
