use std::sync::Arc;
use tokio::sync::RwLock;

use crate::bus::{Hub, Message};
use crate::bus::Scope;
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
        if let Err(e) = self.store.insert(&msg) {
            tracing::warn!(error = %e, "failed to persist message");
        }

        self.hub.write().await.publish(msg);
    }

    pub async fn create_scope(&self, scope: impl Into<Scope>) {
        self.hub.write().await.create_scope(scope.into());
    }
}
