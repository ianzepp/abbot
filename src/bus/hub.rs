use std::collections::HashMap;
use tokio::sync::broadcast;
use super::{Channel, Message};

pub struct Hub {
    channels: HashMap<String, Channel>,
}

impl Hub {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }

    pub fn create_channel(&mut self, name: impl Into<String>) -> &Channel {
        let name = name.into();
        self.channels.entry(name.clone()).or_insert_with(|| Channel::new(name.clone()));
        self.channels.get(&name).unwrap()
    }

    pub fn publish(&self, channel: &str, msg: Message) {
        if let Some(ch) = self.channels.get(channel) {
            ch.publish(msg);
        }
    }

    pub fn subscribe(&self, channel: &str) -> Option<broadcast::Receiver<Message>> {
        self.channels.get(channel).map(|ch| ch.subscribe())
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::respond;

    #[test]
    fn test_hub_create_channel() {
        let mut hub = Hub::new();
        hub.create_channel("#test");
        assert!(hub.subscribe("#test").is_some());
        assert!(hub.subscribe("#nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_hub_pubsub() {
        let mut hub = Hub::new();
        hub.create_channel("#test");

        let mut rx = hub.subscribe("#test").unwrap();

        let msg = respond::chat("alice", "#test", "hello");
        hub.publish("#test", msg);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.sender, "alice");
        assert_eq!(received.text(), Some("hello"));
    }

    #[tokio::test]
    async fn test_hub_multiple_subscribers() {
        let mut hub = Hub::new();
        hub.create_channel("#test");

        let mut rx1 = hub.subscribe("#test").unwrap();
        let mut rx2 = hub.subscribe("#test").unwrap();

        let msg = respond::chat("alice", "#test", "hello");
        hub.publish("#test", msg);

        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();

        assert_eq!(r1.text(), Some("hello"));
        assert_eq!(r2.text(), Some("hello"));
    }
}
