use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use crate::bus::{Hub, Message, respond};

pub struct Client {
    name: String,
    hub: Arc<RwLock<Hub>>,
}

impl Client {
    pub fn new(name: impl Into<String>, hub: Arc<RwLock<Hub>>) -> Self {
        Self {
            name: name.into(),
            hub,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn join(&self, channel: &str) -> Option<broadcast::Receiver<Message>> {
        self.hub.write().await.create_channel(channel);
        self.hub.read().await.subscribe(channel)
    }

    pub async fn say(&self, channel: &str, content: &str) {
        let msg = respond::chat(&self.name, channel, content);
        self.hub.read().await.publish(channel, msg);
    }

    pub async fn publish(&self, msg: Message) {
        let channel = msg.channel.clone();
        self.hub.read().await.publish(&channel, msg);
    }
}
