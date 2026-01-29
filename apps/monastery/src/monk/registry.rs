use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use super::Monk;

/// Registry of all monks and their channel subscriptions.
pub struct Registry {
    monks: HashMap<String, Arc<RwLock<Monk>>>,
    subscriptions: HashMap<String, HashSet<String>>, // channel -> monk_ids
}

impl Registry {
    pub fn new() -> Self {
        Self {
            monks: HashMap::new(),
            subscriptions: HashMap::new(),
        }
    }

    /// Add a monk to the registry.
    pub fn add(&mut self, monk: Monk) {
        let id = monk.id().to_string();
        self.monks.insert(id, Arc::new(RwLock::new(monk)));
    }

    /// Get a monk by ID.
    pub fn get(&self, id: &str) -> Option<Arc<RwLock<Monk>>> {
        self.monks.get(id).cloned()
    }

    /// Remove a monk from the registry.
    pub fn remove(&mut self, id: &str) -> Option<Arc<RwLock<Monk>>> {
        // Remove from all subscriptions
        for subscribers in self.subscriptions.values_mut() {
            subscribers.remove(id);
        }
        self.monks.remove(id)
    }

    /// Subscribe a monk to a channel.
    pub fn subscribe(&mut self, monk_id: &str, channel: &str) {
        self.subscriptions
            .entry(channel.to_string())
            .or_default()
            .insert(monk_id.to_string());
    }

    /// Unsubscribe a monk from a channel.
    pub fn unsubscribe(&mut self, monk_id: &str, channel: &str) {
        if let Some(subscribers) = self.subscriptions.get_mut(channel) {
            subscribers.remove(monk_id);
        }
    }

    /// Get all monks subscribed to a channel.
    pub fn monks_in_channel(&self, channel: &str) -> Vec<Arc<RwLock<Monk>>> {
        let Some(monk_ids) = self.subscriptions.get(channel) else {
            return Vec::new();
        };

        monk_ids
            .iter()
            .filter_map(|id| self.monks.get(id).cloned())
            .collect()
    }

    /// Get all channels a monk is subscribed to.
    pub fn channels_for_monk(&self, monk_id: &str) -> Vec<String> {
        self.subscriptions
            .iter()
            .filter(|(_, monks)| monks.contains(monk_id))
            .map(|(channel, _)| channel.clone())
            .collect()
    }

    /// List all monk IDs.
    pub fn list(&self) -> Vec<String> {
        self.monks.keys().cloned().collect()
    }
}

pub type SharedRegistry = Arc<RwLock<Registry>>;

pub fn new_registry() -> SharedRegistry {
    Arc::new(RwLock::new(Registry::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Store;

    fn test_store() -> Arc<Store> {
        Arc::new(Store::open(":memory:").unwrap())
    }

    #[test]
    fn test_registry_add_and_get() {
        let store = test_store();
        let mut registry = Registry::new();

        let monk = Monk::new("brother-thomas", store, "system".into());
        registry.add(monk);

        assert!(registry.get("brother-thomas").is_some());
        assert!(registry.get("nobody").is_none());
    }

    #[test]
    fn test_registry_subscriptions() {
        let store = test_store();
        let mut registry = Registry::new();

        let monk1 = Monk::new("monk-a", store.clone(), "system".into());
        let monk2 = Monk::new("monk-b", store, "system".into());
        registry.add(monk1);
        registry.add(monk2);

        registry.subscribe("monk-a", "#general");
        registry.subscribe("monk-a", "#ping");
        registry.subscribe("monk-b", "#general");

        let general_monks = registry.monks_in_channel("#general");
        assert_eq!(general_monks.len(), 2);

        let ping_monks = registry.monks_in_channel("#ping");
        assert_eq!(ping_monks.len(), 1);

        let monk_a_channels = registry.channels_for_monk("monk-a");
        assert_eq!(monk_a_channels.len(), 2);
    }

    #[test]
    fn test_registry_unsubscribe() {
        let store = test_store();
        let mut registry = Registry::new();

        let monk = Monk::new("monk-a", store, "system".into());
        registry.add(monk);
        registry.subscribe("monk-a", "#general");

        assert_eq!(registry.monks_in_channel("#general").len(), 1);

        registry.unsubscribe("monk-a", "#general");
        assert_eq!(registry.monks_in_channel("#general").len(), 0);
    }

    #[test]
    fn test_registry_remove() {
        let store = test_store();
        let mut registry = Registry::new();

        let monk = Monk::new("monk-a", store, "system".into());
        registry.add(monk);
        registry.subscribe("monk-a", "#general");
        registry.subscribe("monk-a", "#ping");

        registry.remove("monk-a");

        assert!(registry.get("monk-a").is_none());
        assert_eq!(registry.monks_in_channel("#general").len(), 0);
        assert_eq!(registry.monks_in_channel("#ping").len(), 0);
    }
}
