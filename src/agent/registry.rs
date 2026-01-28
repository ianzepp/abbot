use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};

pub struct AgentHandle {
    pub id: String,
    pub channel: String,
    pub model: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl AgentHandle {
    pub fn new(id: String, channel: String, model: String, shutdown_tx: oneshot::Sender<()>) -> Self {
        Self {
            id,
            channel,
            model,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    pub fn shutdown(&mut self) -> bool {
        if let Some(tx) = self.shutdown_tx.take() {
            tx.send(()).is_ok()
        } else {
            false
        }
    }
}

pub struct AgentRegistry {
    agents: HashMap<String, AgentHandle>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    pub fn register(&mut self, handle: AgentHandle) {
        self.agents.insert(handle.id.clone(), handle);
    }

    pub fn get(&self, id: &str) -> Option<&AgentHandle> {
        self.agents.get(id)
    }

    pub fn dismiss(&mut self, id: &str) -> Option<String> {
        if let Some(mut handle) = self.agents.remove(id) {
            handle.shutdown();
            Some(handle.channel)
        } else {
            None
        }
    }

    pub fn list(&self) -> Vec<(&str, &str, &str)> {
        self.agents.values()
            .map(|h| (h.id.as_str(), h.channel.as_str(), h.model.as_str()))
            .collect()
    }
}

pub type SharedRegistry = Arc<Mutex<AgentRegistry>>;

pub fn new_registry() -> SharedRegistry {
    Arc::new(Mutex::new(AgentRegistry::new()))
}
