use std::sync::Arc;
use tokio::sync::RwLock;
use crate::bus::{Hub, Message};
use crate::chat::Client;

pub struct AgentContext {
    pub client: Client,
}

#[allow(async_fn_in_trait)]
pub trait Agent: Send + Sync {
    fn name(&self) -> &str;
    fn channels(&self) -> Vec<&str>;

    async fn on_message(&self, ctx: &AgentContext, msg: Message);

    async fn run(&self, hub: Arc<RwLock<Hub>>) {
        let client = Client::new(self.name(), hub.clone());
        let ctx = AgentContext { client };

        let mut receivers = Vec::new();
        let mut joined_channels = std::collections::HashSet::new();

        for channel in self.channels() {
            if let Some(rx) = ctx.client.join(channel).await {
                receivers.push((channel.to_string(), rx));
                joined_channels.insert(channel.to_string());
            }
        }

        tracing::info!(agent = self.name(), "started");

        let mut check_counter = 0u32;

        loop {
            // Periodically check for new channels (every ~1 second)
            check_counter += 1;
            if check_counter >= 10 {
                check_counter = 0;
                let all_channels = hub.read().await.channel_names();
                for channel in all_channels {
                    if !joined_channels.contains(&channel) {
                        if let Some(rx) = hub.read().await.subscribe(&channel) {
                            tracing::debug!(agent = self.name(), channel, "joined new channel");
                            receivers.push((channel.clone(), rx));
                            joined_channels.insert(channel);
                        }
                    }
                }
            }

            for (channel, rx) in &mut receivers {
                loop {
                    match rx.try_recv() {
                        Ok(msg) => {
                            if msg.sender != self.name() {
                                tracing::debug!(
                                    agent = self.name(),
                                    channel = channel.as_str(),
                                    from = msg.sender.as_str(),
                                    "received"
                                );
                                self.on_message(&ctx, msg).await;
                            }
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                            tracing::warn!(agent = self.name(), skipped = n, "lagged");
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                            // Don't return, just skip this channel
                            break;
                        }
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::respond;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingAgent {
        name: String,
        count: Arc<AtomicUsize>,
    }

    impl Agent for CountingAgent {
        fn name(&self) -> &str {
            &self.name
        }

        fn channels(&self) -> Vec<&str> {
            vec!["#test"]
        }

        async fn on_message(&self, _ctx: &AgentContext, _msg: Message) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_agent_receives_messages() {
        let hub = Arc::new(RwLock::new(Hub::new()));
        hub.write().await.create_channel("#test");

        let count = Arc::new(AtomicUsize::new(0));
        let agent = CountingAgent {
            name: "counter".to_string(),
            count: count.clone(),
        };

        let hub_clone = hub.clone();
        let handle = tokio::spawn(async move {
            agent.run(hub_clone).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let msg = respond::chat("user", "#test", "hello");
        hub.read().await.publish("#test", msg);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        assert_eq!(count.load(Ordering::SeqCst), 1);

        handle.abort();
    }

    #[tokio::test]
    async fn test_agent_ignores_own_messages() {
        let hub = Arc::new(RwLock::new(Hub::new()));
        hub.write().await.create_channel("#test");

        let count = Arc::new(AtomicUsize::new(0));
        let agent = CountingAgent {
            name: "counter".to_string(),
            count: count.clone(),
        };

        let hub_clone = hub.clone();
        let handle = tokio::spawn(async move {
            agent.run(hub_clone).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let msg = respond::chat("counter", "#test", "my own message");
        hub.read().await.publish("#test", msg);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        assert_eq!(count.load(Ordering::SeqCst), 0);

        handle.abort();
    }
}
