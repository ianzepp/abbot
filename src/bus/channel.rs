// Channel wraps a tokio broadcast channel for a specific scope.
//
// The broadcast channel is chosen over mpsc because we need multiple
// subscribers (services watching a scope) to receive the same messages.
// Capacity of 256 is a balance between memory usage and burst tolerance.

use tokio::sync::broadcast;
use super::Message;
use super::Scope;

const CHANNEL_CAPACITY: usize = 256;

pub struct Channel {
    scope: Scope,
    tx: broadcast::Sender<Message>,
}

impl Channel {
    pub fn new(scope: Scope) -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            scope,
            tx,
        }
    }

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn publish(&self, msg: Message) {
        let _ = self.tx.send(msg);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Message> {
        self.tx.subscribe()
    }
}
