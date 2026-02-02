use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use crate::agent_tools::{describe_tools, hand_tool_specs, head_tool_specs};
use crate::history::Store;
use crate::llm::ToolSpec;

use super::{PluginManager, build_environment_layer, build_network_layer};

#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub workspace_root: PathBuf,
    pub commandments_md: String,
    pub environment_md: String,

    pub plugins: PluginManager,

    pub head_tools: Vec<ToolSpec>,
    pub head_tools_md: String,

    pub hand_tools: Vec<ToolSpec>,
    pub hand_tools_md: String,

    pub external_tools: Vec<ToolSpec>,
    pub external_tool_names: HashSet<String>,
    pub external_name_map: HashMap<String, String>,
}

fn merge_tools(mut base: Vec<ToolSpec>, plugin: Vec<ToolSpec>) -> Vec<ToolSpec> {
    let plugin_names: HashSet<String> = plugin.iter().map(|t| t.function.name.clone()).collect();
    base.retain(|t| !plugin_names.contains(&t.function.name));
    base.extend(plugin);
    base
}

impl RuntimeSnapshot {
    pub fn build(workspace_root: PathBuf, store: Option<&Store>) -> Self {
        let commandments_md = include_str!("commandments.md").to_string();
        let environment_md = format!(
            "{}\n\n{}",
            build_environment_layer(Some(&workspace_root)),
            build_network_layer()
        );

        let plugins = PluginManager::load_for_workspace_root(&workspace_root);

        // Head
        let head_tools = merge_tools(head_tool_specs(), plugins.head_tool_specs());
        let mut head_tools_md = describe_tools(&head_tools);
        let playbooks = plugins.head_playbooks_md();
        if !playbooks.trim().is_empty() {
            head_tools_md.push_str("\n\n");
            head_tools_md.push_str(playbooks.trim());
        }

        // Hand
        let hand_tools = merge_tools(hand_tool_specs(), plugins.hand_tool_specs());
        let mut hand_tools_md = describe_tools(&hand_tools);
        let playbooks = plugins.hand_playbooks_md();
        if !playbooks.trim().is_empty() {
            hand_tools_md.push_str("\n\n");
            hand_tools_md.push_str(playbooks.trim());
        }

        let (external_tools, external_tool_names, external_name_map) =
            Self::build_external_tools(store);

        Self {
            workspace_root,
            commandments_md,
            environment_md,
            plugins,
            head_tools,
            head_tools_md,
            hand_tools,
            hand_tools_md,
            external_tools,
            external_tool_names,
            external_name_map,
        }
    }

    fn build_external_tools(
        store: Option<&Store>,
    ) -> (Vec<ToolSpec>, HashSet<String>, HashMap<String, String>) {
        let mut external_tools = Vec::new();
        let mut external_tool_names = HashSet::new();
        let mut external_name_map = HashMap::new();

        let Some(store) = store else {
            return (external_tools, external_tool_names, external_name_map);
        };

        if let Ok(ext) = store.list_tools("main", "external") {
            for t in ext {
                if let Ok(schema) = serde_json::from_str::<serde_json::Value>(&t.schema_json) {
                    let internal_name = format!("client__{}", t.name);
                    external_name_map.insert(internal_name.clone(), t.name.clone());
                    external_tools.push(ToolSpec::function(
                        internal_name.clone(),
                        if t.description.trim().is_empty() {
                            t.summary.clone()
                        } else {
                            t.description.clone()
                        },
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
    refresh_lock: Mutex<()>,
    store: Option<Arc<Store>>,
}

impl std::fmt::Debug for SnapshotManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotManager")
            .field("inner", &self.inner)
            .field("refresh_lock", &self.refresh_lock)
            .finish_non_exhaustive()
    }
}

impl SnapshotManager {
    pub fn new(workspace_root: PathBuf, store: Option<Arc<Store>>) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(RuntimeSnapshot::build(
                workspace_root,
                store.as_ref().map(|s| s.as_ref()),
            )),
            refresh_lock: Mutex::new(()),
            store,
        })
    }

    pub fn refresh(&self) {
        let _guard = self
            .refresh_lock
            .lock()
            .expect("snapshot refresh lock poisoned");

        let workspace_root = self
            .inner
            .read()
            .expect("snapshot lock poisoned")
            .workspace_root
            .clone();
        let next = RuntimeSnapshot::build(workspace_root, self.store.as_ref().map(|s| s.as_ref()));
        *self.inner.write().expect("snapshot lock poisoned") = next;
    }

    pub fn get(&self) -> RuntimeSnapshot {
        self.inner.read().expect("snapshot lock poisoned").clone()
    }
}
