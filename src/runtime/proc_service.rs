// ProcService is a central registry for runtime state.
//
// Services push their state here; the proc tool reads from here.
// Path-based addressing with explicit kind enum for safety.
// Heads get full CRUD access, hands are read-only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{Notify, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcKind {
    Tasks,
    Needs,
    Heads,
    Hands,
    Tools,
    Config,
}

impl ProcKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tasks => "tasks",
            Self::Needs => "needs",
            Self::Heads => "heads",
            Self::Hands => "hands",
            Self::Tools => "tools",
            Self::Config => "config",
        }
    }

    pub fn all() -> &'static [ProcKind] {
        &[
            Self::Tasks,
            Self::Needs,
            Self::Heads,
            Self::Hands,
            Self::Tools,
            Self::Config,
        ]
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "tasks" => Some(Self::Tasks),
            "needs" => Some(Self::Needs),
            "heads" => Some(Self::Heads),
            "hands" => Some(Self::Hands),
            "tools" => Some(Self::Tools),
            "config" => Some(Self::Config),
            _ => None,
        }
    }
}

pub struct ProcService {
    entries: BTreeMap<String, Value>,
    watchers: HashMap<String, Arc<Notify>>,
}

pub type ProcHandle = Arc<RwLock<ProcService>>;

impl ProcService {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            watchers: HashMap::new(),
        }
    }

    pub fn handle(self) -> ProcHandle {
        Arc::new(RwLock::new(self))
    }

    /// Select a single entry. Returns None if not found.
    pub fn select(&self, kind: ProcKind, path: &str) -> Option<Value> {
        let key = make_key(kind, path);
        self.entries.get(&key).cloned()
    }

    /// List top-level entries under a kind.
    /// Returns immediate child names, not full paths.
    pub fn list(&self, kind: ProcKind) -> Vec<String> {
        let prefix = format!("{}/", kind.as_str());

        let mut children = Vec::new();
        let mut seen = HashSet::new();

        for key in self.entries.keys() {
            if let Some(suffix) = key.strip_prefix(&prefix) {
                let child = suffix.split('/').next().unwrap_or("");
                if !child.is_empty() && seen.insert(child.to_string()) {
                    children.push(child.to_string());
                }
            }
        }

        children
    }

    /// Create an entry at a path. Overwrites if exists.
    /// Empty path is a no-op. Notifies watchers.
    pub fn create(&mut self, kind: ProcKind, path: &str, value: Value) {
        let path = normalize_path(path);
        if path.is_empty() {
            return;
        }
        let key = format!("{}/{}", kind.as_str(), path);
        self.entries.insert(key.clone(), value);
        self.notify(&key);
    }

    /// Update a value at a path by merging (for objects).
    /// Creates if not exists. Empty path is a no-op. Notifies watchers.
    pub fn update(&mut self, kind: ProcKind, path: &str, patch: Value) {
        let path = normalize_path(path);
        if path.is_empty() {
            return;
        }
        let key = format!("{}/{}", kind.as_str(), path);

        match self.entries.get_mut(&key) {
            Some(Value::Object(existing)) if patch.is_object() => {
                if let Value::Object(patch_map) = patch {
                    for (k, v) in patch_map {
                        existing.insert(k, v);
                    }
                }
            }
            Some(existing) => {
                *existing = patch;
            }
            None => {
                self.entries.insert(key.clone(), patch);
            }
        }
        self.notify(&key);
    }

    /// Delete an entry and its children.
    /// Empty path is a no-op (cannot delete entire kind). Notifies watchers.
    pub fn delete(&mut self, kind: ProcKind, path: &str) -> bool {
        let path = normalize_path(path);
        if path.is_empty() {
            return false;
        }

        let key = format!("{}/{}", kind.as_str(), path);
        let removed = self.entries.remove(&key).is_some();

        let prefix = format!("{}/", key);
        let children: Vec<String> = self
            .entries
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();

        for child in &children {
            self.entries.remove(child);
            self.notify(child);
        }

        if removed || !children.is_empty() {
            self.notify(&key);
            true
        } else {
            false
        }
    }

    /// Check if a path exists (exact match).
    pub fn exists(&self, kind: ProcKind, path: &str) -> bool {
        let key = make_key(kind, path);
        self.entries.contains_key(&key)
    }

    /// Get a Notify handle for an entity. Await `notified()` outside the lock.
    /// The notify fires on create, update, or delete of this entity.
    pub fn watcher(&mut self, kind: ProcKind, path: &str) -> Arc<Notify> {
        let key = make_key(kind, path);
        self.watchers
            .entry(key)
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    /// Notify watchers for a key. Called internally by create/update/delete.
    fn notify(&self, key: &str) {
        if let Some(notify) = self.watchers.get(key) {
            notify.notify_waiters();
        }
    }
}

impl Default for ProcService {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_path(path: &str) -> String {
    path.trim_matches('/').to_string()
}

fn make_key(kind: ProcKind, path: &str) -> String {
    let path = normalize_path(path);
    if path.is_empty() {
        kind.as_str().to_string()
    } else {
        format!("{}/{}", kind.as_str(), path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_create_and_select() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "abc", json!({"status": "pending"}));

        let val = proc.select(ProcKind::Tasks, "abc");
        assert_eq!(val, Some(json!({"status": "pending"})));
    }

    #[test]
    fn test_create_empty_path_noop() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "", json!({"bad": "data"}));

        assert!(proc.list(ProcKind::Tasks).is_empty());
    }

    #[test]
    fn test_select_normalizes_path() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "abc", json!({"status": "pending"}));

        assert_eq!(
            proc.select(ProcKind::Tasks, "/abc/"),
            Some(json!({"status": "pending"}))
        );
    }

    #[test]
    fn test_list() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "a", json!({}));
        proc.create(ProcKind::Tasks, "b", json!({}));
        proc.create(ProcKind::Needs, "x", json!({}));

        let mut tasks = proc.list(ProcKind::Tasks);
        tasks.sort();
        assert_eq!(tasks, vec!["a", "b"]);

        let needs = proc.list(ProcKind::Needs);
        assert_eq!(needs, vec!["x"]);
    }

    #[test]
    fn test_list_with_nested_entries() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "a", json!({}));
        proc.create(ProcKind::Tasks, "a/logs/1", json!({}));
        proc.create(ProcKind::Tasks, "b/meta", json!({}));

        // list() only returns top-level entries
        let mut tasks = proc.list(ProcKind::Tasks);
        tasks.sort();
        assert_eq!(tasks, vec!["a", "b"]);
    }

    #[test]
    fn test_update_merges_objects() {
        let mut proc = ProcService::new();
        proc.create(
            ProcKind::Tasks,
            "a",
            json!({"status": "pending", "goal": "test"}),
        );
        proc.update(ProcKind::Tasks, "a", json!({"status": "running"}));

        let val = proc.select(ProcKind::Tasks, "a").unwrap();
        assert_eq!(val["status"], "running");
        assert_eq!(val["goal"], "test");
    }

    #[test]
    fn test_update_empty_path_noop() {
        let mut proc = ProcService::new();
        proc.update(ProcKind::Tasks, "", json!({"bad": "data"}));

        assert!(proc.list(ProcKind::Tasks).is_empty());
    }

    #[test]
    fn test_update_creates_if_missing() {
        let mut proc = ProcService::new();
        proc.update(ProcKind::Tasks, "new", json!({"status": "pending"}));

        assert_eq!(
            proc.select(ProcKind::Tasks, "new"),
            Some(json!({"status": "pending"}))
        );
    }

    #[test]
    fn test_delete_single() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "a", json!({}));
        proc.create(ProcKind::Tasks, "b", json!({}));

        assert!(proc.delete(ProcKind::Tasks, "a"));
        assert!(proc.select(ProcKind::Tasks, "a").is_none());
        assert!(proc.select(ProcKind::Tasks, "b").is_some());
    }

    #[test]
    fn test_delete_cascades() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "a", json!({}));
        proc.create(ProcKind::Tasks, "a/logs/1", json!({}));
        proc.create(ProcKind::Tasks, "a/logs/2", json!({}));
        proc.create(ProcKind::Tasks, "b", json!({}));

        proc.delete(ProcKind::Tasks, "a");

        assert!(proc.select(ProcKind::Tasks, "a").is_none());
        assert!(proc.select(ProcKind::Tasks, "a/logs/1").is_none());
        assert!(proc.select(ProcKind::Tasks, "a/logs/2").is_none());
        assert!(proc.select(ProcKind::Tasks, "b").is_some());
    }

    #[test]
    fn test_delete_empty_path_noop() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "a", json!({}));
        proc.create(ProcKind::Tasks, "b", json!({}));

        assert!(!proc.delete(ProcKind::Tasks, ""));
        assert!(proc.select(ProcKind::Tasks, "a").is_some());
        assert!(proc.select(ProcKind::Tasks, "b").is_some());
    }

    #[test]
    fn test_delete_nonexistent() {
        let mut proc = ProcService::new();
        assert!(!proc.delete(ProcKind::Tasks, "nope"));
    }

    #[test]
    fn test_exists() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "a", json!({}));

        assert!(proc.exists(ProcKind::Tasks, "a"));
        assert!(!proc.exists(ProcKind::Tasks, "b"));
        assert!(!proc.exists(ProcKind::Needs, "a"));
    }

    #[test]
    fn test_kinds_are_isolated() {
        let mut proc = ProcService::new();
        proc.create(ProcKind::Tasks, "x", json!({"from": "tasks"}));
        proc.create(ProcKind::Needs, "x", json!({"from": "needs"}));

        assert_eq!(
            proc.select(ProcKind::Tasks, "x"),
            Some(json!({"from": "tasks"}))
        );
        assert_eq!(
            proc.select(ProcKind::Needs, "x"),
            Some(json!({"from": "needs"}))
        );

        proc.delete(ProcKind::Tasks, "x");
        assert!(proc.select(ProcKind::Tasks, "x").is_none());
        assert!(proc.select(ProcKind::Needs, "x").is_some());
    }

    #[test]
    fn test_kind_from_str() {
        assert_eq!(ProcKind::from_str("tasks"), Some(ProcKind::Tasks));
        assert_eq!(ProcKind::from_str("needs"), Some(ProcKind::Needs));
        assert_eq!(ProcKind::from_str("invalid"), None);
    }

    #[tokio::test]
    async fn test_watcher_notified_on_update() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;

        let proc = ProcService::new().handle();

        // Create initial entry
        proc.write()
            .await
            .create(ProcKind::Tasks, "abc", json!({"status": "pending"}));

        // Get watcher before update
        let notify = proc.write().await.watcher(ProcKind::Tasks, "abc");
        let notified = Arc::new(AtomicBool::new(false));
        let notified_clone = notified.clone();

        // Spawn task to wait for notification
        let handle = tokio::spawn(async move {
            notify.notified().await;
            notified_clone.store(true, Ordering::SeqCst);
        });

        // Give the spawned task time to start waiting
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!notified.load(Ordering::SeqCst));

        // Update triggers notification
        proc.write()
            .await
            .update(ProcKind::Tasks, "abc", json!({"status": "done"}));

        // Wait for spawned task
        tokio::time::timeout(Duration::from_millis(100), handle)
            .await
            .expect("timeout")
            .expect("join");

        assert!(notified.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_watcher_notified_on_delete() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;

        let proc = ProcService::new().handle();

        proc.write().await.create(ProcKind::Tasks, "abc", json!({}));

        let notify = proc.write().await.watcher(ProcKind::Tasks, "abc");
        let notified = Arc::new(AtomicBool::new(false));
        let notified_clone = notified.clone();

        let handle = tokio::spawn(async move {
            notify.notified().await;
            notified_clone.store(true, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(10)).await;

        proc.write().await.delete(ProcKind::Tasks, "abc");

        tokio::time::timeout(Duration::from_millis(100), handle)
            .await
            .expect("timeout")
            .expect("join");

        assert!(notified.load(Ordering::SeqCst));
    }
}
