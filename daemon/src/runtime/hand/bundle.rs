use std::sync::Arc;

use crate::hal::llm::UnifiedMessage as Message;
use crate::history::Store;
use crate::runtime::SnapshotManager;
use crate::runtime::{atomic_write_file_0600, read_optional_file, workspace_head_memory};
use std::path::PathBuf;

use crate::runtime::{SystemBundler, SystemSlot, TarsDials};

pub struct HandBundleConfig {
    pub task_id: String,
    pub head_id: String,
    pub prompt: String,
    pub input: String,
    pub traits: Vec<String>,
    pub max_iters: usize,
}

impl HandBundleConfig {
    pub fn new(
        task_id: impl Into<String>,
        head_id: impl Into<String>,
        prompt: impl Into<String>,
        input: impl Into<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            head_id: head_id.into(),
            prompt: prompt.into(),
            input: input.into(),
            traits: Vec::new(),
            max_iters: 24,
        }
    }

    pub fn with_traits(mut self, traits: Vec<String>) -> Self {
        self.traits = traits;
        self
    }

    pub fn with_max_iters(mut self, max_iters: usize) -> Self {
        self.max_iters = max_iters;
        self
    }
}

pub struct HandBundleBuilder {
    store: Arc<Store>,
    workspace_root: PathBuf,
    system: String,
    snapshot: Arc<SnapshotManager>,
}

impl HandBundleBuilder {
    pub async fn new(store: Arc<Store>, workspace_root: PathBuf) -> Self {
        let snapshot = SnapshotManager::new(workspace_root.clone(), Some(store.clone())).await;
        Self::new_with_snapshot(store, workspace_root, snapshot)
    }

    pub fn new_with_snapshot(
        store: Arc<Store>,
        workspace_root: PathBuf,
        snapshot: Arc<SnapshotManager>,
    ) -> Self {
        let system = include_str!("hand_system.md");
        Self {
            store,
            workspace_root,
            system: system.to_string(),
            snapshot,
        }
    }

    pub async fn build(&self, cfg: &HandBundleConfig) -> Vec<Message> {
        let mut messages = Vec::new();

        let snap = self.snapshot.get();

        // System message: core + commandments + tools + env + traits (near end)
        let system_content = SystemBundler::new()
            .with_layer(SystemSlot::Core, self.system.clone())
            .with_layer(SystemSlot::Commandments, snap.commandments_md.trim())
            .with_tools_section(SystemSlot::ToolsPrimary, "Tools", snap.hand_tools_md.trim())
            .with_layer(SystemSlot::Environment, snap.environment_md.trim())
            .with_tone(&TarsDials::default(), &cfg.traits)
            .build();
        messages.push(Message::system(system_content));

        // Initial user message: STM context + task prompt and input
        let stm = self.load_head_stm(&cfg.head_id).await;
        let initial_prompt = build_initial_prompt(&stm, &cfg.prompt, &cfg.input, cfg.max_iters);
        messages.push(Message::user(initial_prompt));

        // Load conversation history from DB
        let history = self
            .store
            .get_hand_execs(&cfg.task_id)
            .await
            .unwrap_or_default();

        for record in history {
            // Add assistant turn (hand's thought/response)
            if !record.hand_thought.is_empty() {
                messages.push(Message::assistant(record.hand_thought.clone()));
            }

            // Add user turn (tool result)
            let tool_result = if record.success {
                format!("[Tool {} completed]\n{}", record.tool, record.output)
            } else {
                format!("[Tool {} failed]\n{}", record.tool, record.output)
            };
            messages.push(Message::user(tool_result));
        }

        messages
    }

    async fn load_head_stm(&self, head_id: &str) -> String {
        let path = workspace_head_memory(&self.workspace_root, head_id);

        if let Ok(Some(content)) = read_optional_file(&path) {
            return content;
        }

        // One-time migration from legacy DB location.
        let legacy = self.store.get_head_stm(head_id).await.unwrap_or_default();
        if !legacy.trim().is_empty() {
            let _ = atomic_write_file_0600(&path, legacy.trim());
            return legacy;
        }

        String::new()
    }
}

fn build_initial_prompt(stm: &str, prompt: &str, input: &str, max_iters: usize) -> String {
    let mut out = String::new();

    if !stm.trim().is_empty() {
        out.push_str("CONTEXT (from head's short-term memory):\n");
        out.push_str(stm.trim());
        out.push_str("\n\n");
    }

    out.push_str("TASK\n");
    out.push_str(prompt.trim());
    out.push('\n');
    if !input.trim().is_empty() {
        out.push_str("input:\n");
        out.push_str(input.trim());
        out.push('\n');
    }

    out.push_str(&format!(
        "\nBUDGET: You have {} tool iterations for this task. Plan accordingly.\n",
        max_iters
    ));
    out
}
