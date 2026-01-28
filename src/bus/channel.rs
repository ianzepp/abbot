use tokio::sync::broadcast;
use super::Message;

const CHANNEL_CAPACITY: usize = 256;

pub struct Channel {
    name: String,
    tx: broadcast::Sender<Message>,
}

impl Channel {
    pub fn new(name: impl Into<String>) -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            name: name.into(),
            tx,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn publish(&self, msg: Message) {
        let _ = self.tx.send(msg);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Message> {
        self.tx.subscribe()
    }
}
