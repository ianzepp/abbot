use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use crate::agent_tools::{describe_tools, hand_tool_specs, head_tool_specs};
use crate::llm::ToolSpec;

use super::{build_environment_layer, PluginManager};

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
}

fn merge_tools(mut base: Vec<ToolSpec>, plugin: Vec<ToolSpec>) -> Vec<ToolSpec> {
    let plugin_names: HashSet<String> = plugin.iter().map(|t| t.function.name.clone()).collect();
    base.retain(|t| !plugin_names.contains(&t.function.name));
    base.extend(plugin);
    base
}

impl RuntimeSnapshot {
    pub fn build(workspace_root: PathBuf) -> Self {
        let commandments_md = include_str!("commandments.md").to_string();
        let environment_md = build_environment_layer(Some(&workspace_root));

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

        Self {
            workspace_root,
            commandments_md,
            environment_md,
            plugins,
            head_tools,
            head_tools_md,
            hand_tools,
            hand_tools_md,
        }
    }
}

#[derive(Debug)]
pub struct SnapshotManager {
    inner: RwLock<RuntimeSnapshot>,
    refresh_lock: Mutex<()>,
}

impl SnapshotManager {
    pub fn new(workspace_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(RuntimeSnapshot::build(workspace_root)),
            refresh_lock: Mutex::new(()),
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
        let next = RuntimeSnapshot::build(workspace_root);
        *self.inner.write().expect("snapshot lock poisoned") = next;
    }

    pub fn get(&self) -> RuntimeSnapshot {
        self.inner.read().expect("snapshot lock poisoned").clone()
    }
}
