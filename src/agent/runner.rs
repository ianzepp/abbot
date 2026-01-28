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
        for channel in self.channels() {
            if let Some(rx) = ctx.client.join(channel).await {
                receivers.push((channel.to_string(), rx));
            }
        }

        tracing::info!(agent = self.name(), "started");

        loop {
            for (channel, rx) in &mut receivers {
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
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                        tracing::warn!(agent = self.name(), skipped = n, "lagged");
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                        tracing::error!(agent = self.name(), "channel closed");
                        return;
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
