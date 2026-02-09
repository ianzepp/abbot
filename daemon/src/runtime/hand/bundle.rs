use std::path::PathBuf;
use std::sync::Arc;

use crate::hal::llm::UnifiedMessage as Message;
use crate::history::Store;
use crate::runtime::SnapshotManager;
use crate::runtime::{SystemBundler, SystemSlot};

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
    system: String,
    snapshot: Arc<SnapshotManager>,
}

impl HandBundleBuilder {
    pub async fn new(store: Arc<Store>, workspace_root: PathBuf) -> Self {
        let snapshot = SnapshotManager::new(workspace_root, Some(store.clone())).await;
        Self::new_with_snapshot(store, snapshot)
    }

    pub fn new_with_snapshot(store: Arc<Store>, snapshot: Arc<SnapshotManager>) -> Self {
        let system = include_str!("../../prompts/hand/system.md");
        Self {
            store,
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
            .with_tone(&cfg.traits)
            .build();
        messages.push(Message::system(system_content));

        // Initial user message: task prompt and input
        let initial_prompt = build_initial_prompt(&cfg.prompt, &cfg.input, cfg.max_iters);
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
}

fn build_initial_prompt(prompt: &str, input: &str, max_iters: usize) -> String {
    let input_section = if input.trim().is_empty() {
        String::new()
    } else {
        format!("input:\n{}\n", input.trim())
    };

    include_str!("../../prompts/hand/initial_prompt.md")
        .replace("{prompt}", prompt.trim())
        .replace("{input}", &input_section)
        .replace("{max_iters}", &max_iters.to_string())
}
