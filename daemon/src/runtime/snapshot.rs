use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::hal::llm::ToolSpec;
use crate::history::Store;
use crate::syscalls::dispatch::{describe_tools, hand_catalog, head_catalog};

use super::{build_environment_layer, build_network_layer};

#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub workspace_root: PathBuf,
    pub commandments_md: String,
    pub environment_md: String,

    pub head_tools: Vec<ToolSpec>,
    pub head_tools_md: String,

    pub hand_tools: Vec<ToolSpec>,
    pub hand_tools_md: String,

    pub external_tools: Vec<ToolSpec>,
    pub external_tool_names: HashSet<String>,
    pub external_name_map: HashMap<String, String>,
}

impl RuntimeSnapshot {
    pub async fn build(workspace_root: PathBuf, store: Option<&Store>) -> Self {
        let commandments_md = include_str!("commandments.md").to_string();
        let environment_md = format!("{}\n\n{}", build_environment_layer(), build_network_layer());

        let head_tools = head_catalog();
        let head_tools_md = describe_tools(&head_tools);

        let hand_tools = hand_catalog();
        let hand_tools_md = describe_tools(&hand_tools);

        let (external_tools, external_tool_names, external_name_map) =
            Self::build_external_tools(store).await;

        Self {
            workspace_root,
            commandments_md,
            environment_md,
            head_tools,
            head_tools_md,
            hand_tools,
            hand_tools_md,
            external_tools,
            external_tool_names,
            external_name_map,
        }
    }

    async fn build_external_tools(
        store: Option<&Store>,
    ) -> (Vec<ToolSpec>, HashSet<String>, HashMap<String, String>) {
        let mut external_tools = Vec::new();
        let mut external_tool_names = HashSet::new();
        let mut external_name_map = HashMap::new();

        let Some(store) = store else {
            return (external_tools, external_tool_names, external_name_map);
        };

        if let Ok(ext) = store.list_tools("main", "external").await {
            for t in ext {
                if let Ok(schema) = serde_json::from_str::<serde_json::Value>(&t.schema_json) {
                    let internal_name = format!("user__{}", t.name);
                    external_name_map.insert(internal_name.clone(), t.name.clone());
                    external_tools.push(ToolSpec::function(
                        internal_name.clone(),
                        // Keep external tool specs terse; full description is available via head__tool_explain.
                        t.summary.clone(),
                        schema,
                    ));
                    external_tool_names.insert(internal_name);
                }
            }
        }

        (external_tools, external_tool_names, external_name_map)
    }
}

pub struct SnapshotManager {
    inner: RwLock<RuntimeSnapshot>,
    refresh_lock: tokio::sync::Mutex<()>,
    store: Option<Arc<Store>>,
}

impl std::fmt::Debug for SnapshotManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotManager")
            .field("inner", &self.inner)
            .field("refresh_lock", &"<tokio::Mutex>")
            .finish_non_exhaustive()
    }
}

impl SnapshotManager {
    pub async fn new(workspace_root: PathBuf, store: Option<Arc<Store>>) -> Arc<Self> {
        let snapshot =
            RuntimeSnapshot::build(workspace_root, store.as_ref().map(|s| s.as_ref())).await;
        Arc::new(Self {
            inner: RwLock::new(snapshot),
            refresh_lock: tokio::sync::Mutex::new(()),
            store,
        })
    }

    pub async fn refresh(&self) {
        let _guard = self.refresh_lock.lock().await;

        let workspace_root = self
            .inner
            .read()
            .expect("snapshot lock poisoned")
            .workspace_root
            .clone();
        let next =
            RuntimeSnapshot::build(workspace_root, self.store.as_ref().map(|s| s.as_ref())).await;
        *self.inner.write().expect("snapshot lock poisoned") = next;
    }

    pub fn get(&self) -> RuntimeSnapshot {
        self.inner.read().expect("snapshot lock poisoned").clone()
    }
}
