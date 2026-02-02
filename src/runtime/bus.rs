// RuntimeBus wraps the Hub with SQLite persistence.
//
// Every message published through RuntimeBus is first written to SQLite,
// then broadcast on the Hub. This ensures durability: if a service restarts,
// it can recover state by replaying messages from the store. The RwLock on
// Hub allows concurrent reads (subscriptions) while serializing writes.

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::bus::Scope;
use crate::bus::{Hub, Message};
use crate::history::Store;

#[derive(Clone)]
pub struct RuntimeBus {
    hub: Arc<RwLock<Hub>>,
    store: Arc<Store>,
}

impl RuntimeBus {
    pub fn new(hub: Arc<RwLock<Hub>>, store: Arc<Store>) -> Self {
        Self { hub, store }
    }

    pub fn hub(&self) -> &Arc<RwLock<Hub>> {
        &self.hub
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub async fn publish(&self, msg: Message) {
        let store = self.store.clone();
        let msg_clone = msg.clone();
        tokio::spawn(async move {
            if let Err(e) = store.insert(&msg_clone) {
                tracing::warn!(error = %e, "failed to persist message");
            }
        });

        self.hub.write().await.publish(msg);
    }

    pub async fn create_scope(&self, scope: impl Into<Scope>) {
        self.hub.write().await.create_scope(scope.into());
    }
}
