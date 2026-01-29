use std::sync::Arc;
use tokio::sync::RwLock;
use crate::bus::{Hub, Message};
use super::{Monk, SharedRegistry};

/// The event loop that dispatches messages to monks.
pub struct Runner {
    hub: Arc<RwLock<Hub>>,
    registry: SharedRegistry,
}

impl Runner {
    pub fn new(hub: Arc<RwLock<Hub>>, registry: SharedRegistry) -> Self {
        Self { hub, registry }
    }

    /// Run the event loop, dispatching messages to subscribed monks.
    pub async fn run(&self) {
        // Subscribe to all channels we care about
        let channels = {
            let hub = self.hub.read().await;
            hub.channel_names()
        };

        let mut receivers = Vec::new();
        for channel in &channels {
            if let Some(rx) = self.hub.read().await.subscribe(channel) {
                receivers.push((channel.clone(), rx));
            }
        }

        tracing::info!(channels = ?channels, "runner started");

        loop {
            // Check each channel for messages
            for (channel, rx) in &mut receivers {
                match rx.try_recv() {
                    Ok(msg) => {
                        self.dispatch(channel, msg).await;
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                        tracing::warn!(channel, skipped = n, "runner lagged");
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                        tracing::warn!(channel, "channel closed");
                    }
                }
            }

            // Check for new channels periodically
            // TODO: could be smarter about this

            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    }

    /// Dispatch a message to all monks subscribed to the channel.
    async fn dispatch(&self, channel: &str, msg: Message) {
        // Don't dispatch messages from monks back to monks (avoid loops)
        // Actually, monks might want to see each other's messages
        // Let's just skip messages from _heartbeat for now
        if msg.sender.starts_with('_') {
            // System message, still dispatch
        }

        let monks = {
            let registry = self.registry.read().await;
            registry.monks_in_channel(channel)
        };

        if monks.is_empty() {
            return;
        }

        tracing::debug!(
            channel,
            sender = msg.sender,
            monk_count = monks.len(),
            "dispatching message"
        );

        // Invoke all monks concurrently - fire and forget
        // Each monk handles messages independently; don't block the runner
        for monk in monks {
            let msg = msg.clone();
            let channel = channel.to_string();

            tokio::spawn(async move {
                let monk = monk.read().await;

                // Skip if the monk sent this message (don't respond to self)
                if monk.id() == msg.sender {
                    return;
                }

                monk.on_message(&channel, &msg).await;
            });
        }
    }
}
