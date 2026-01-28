use std::sync::Arc;
use tokio::sync::RwLock;
use crate::bus::{Hub, respond};

pub struct Room {
    hub: Arc<RwLock<Hub>>,
}

impl Room {
    pub fn new(hub: Arc<RwLock<Hub>>) -> Self {
        Self { hub }
    }

    pub async fn join(&self, channel: &str) {
        self.hub.write().await.create_channel(channel);
    }

    pub async fn say(&self, sender: &str, channel: &str, content: &str) {
        let msg = respond::chat(sender, channel, content);
        self.hub.read().await.publish(channel, msg);
    }
}
