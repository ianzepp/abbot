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
