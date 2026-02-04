use std::sync::Arc;

use crate::history::Store;
use crate::llm::{ChatMessage, Role};
use crate::runtime::SnapshotManager;
use crate::runtime::{atomic_write_file_0600, read_optional_file, workspace_head_memory};
use std::path::PathBuf;

use super::{FeverMode, GenerationMode, SystemBundler, SystemSlot, TarsDials};

/// Autist mode controls Hand execution style.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AutistMode {
    /// No autist mode - default behavior
    #[default]
    None,
    /// ADHD - scattered, starts many things, hyperfocus on tangents
    Adhd,
    /// Neurotypical - normal execution, follows instructions
    Neurotypical,
    /// Autist - obsessive, perfectionist, fixes things you didn't ask about
    Autist,
    /// Full Retard - no guardrails, YOLO, chaotic
    FullRetard,
}

impl AutistMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('-', "").replace('_', "").as_str() {
            "adhd" => Some(Self::Adhd),
            "neurotypical" | "nt" => Some(Self::Neurotypical),
            "autist" => Some(Self::Autist),
            "fullretard" | "retard" => Some(Self::FullRetard),
            "" | "none" => Some(Self::None),
            _ => None,
        }
    }
}

pub struct HandBundleConfig {
    pub task_id: String,
    pub head_id: String,
    pub goal: String,
    pub input: String,
    pub autist: AutistMode,
    pub filter: crate::runtime::FilterMode,
    pub poverty: crate::runtime::PovertyMode,
}

impl HandBundleConfig {
    pub fn new(
        task_id: impl Into<String>,
        head_id: impl Into<String>,
        goal: impl Into<String>,
        input: impl Into<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            head_id: head_id.into(),
            goal: goal.into(),
            input: input.into(),
            autist: AutistMode::None,
            filter: crate::runtime::FilterMode::None,
            poverty: crate::runtime::PovertyMode::None,
        }
    }

    pub fn with_autist(mut self, autist: AutistMode) -> Self {
        self.autist = autist;
        self
    }

    pub fn with_filter(mut self, filter: crate::runtime::FilterMode) -> Self {
        self.filter = filter;
        self
    }

    pub fn with_poverty(mut self, poverty: crate::runtime::PovertyMode) -> Self {
        self.poverty = poverty;
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
    pub fn new(store: Arc<Store>, workspace_root: PathBuf) -> Self {
        let snapshot = SnapshotManager::new(workspace_root.clone(), Some(store.clone()));
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

    pub fn build(&self, cfg: &HandBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        let snap = self.snapshot.get();

        // System message: core + commandments + tools + env + traits (near end)
        let system_content = SystemBundler::new()
            .with_layer(SystemSlot::Core, self.system.clone())
            .with_layer(SystemSlot::Commandments, snap.commandments_md.trim())
            .with_tools_section(SystemSlot::ToolsPrimary, "Tools", snap.hand_tools_md.trim())
            .with_layer(SystemSlot::Environment, snap.environment_md.trim())
            .with_traits_and_tars(
                &TarsDials::default(),
                &FeverMode::None,
                &GenerationMode::None,
                &cfg.autist,
                &cfg.filter,
                &cfg.poverty,
            )
            .build();
        messages.push(ChatMessage::new(Role::System, system_content));

        // Initial user message: STM context + task goal and input
        let stm = self.load_head_stm(&cfg.head_id);
        let initial_prompt = build_initial_prompt(&stm, &cfg.goal, &cfg.input);
        messages.push(ChatMessage::new(Role::User, initial_prompt));

        // Load conversation history from DB
        let history = self.store.get_hand_execs(&cfg.task_id).unwrap_or_default();

        for record in history {
            // Add assistant turn (hand's thought/response)
            if !record.hand_thought.is_empty() {
                messages.push(ChatMessage::new(
                    Role::Assistant,
                    record.hand_thought.clone(),
                ));
            }

            // Add user turn (tool result)
            let tool_result = if record.success {
                format!("[Tool {} completed]\n{}", record.tool, record.output)
            } else {
                format!("[Tool {} failed]\n{}", record.tool, record.output)
            };
            messages.push(ChatMessage::new(Role::User, tool_result));
        }

        messages
    }

    fn load_head_stm(&self, head_id: &str) -> String {
        let path = workspace_head_memory(&self.workspace_root, head_id);

        if let Ok(Some(content)) = read_optional_file(&path) {
            return content;
        }

        // One-time migration from legacy DB location.
        let legacy = self.store.get_head_stm(head_id).unwrap_or_default();
        if !legacy.trim().is_empty() {
            let _ = atomic_write_file_0600(&path, legacy.trim());
            return legacy;
        }

        String::new()
    }
}

fn build_initial_prompt(stm: &str, goal: &str, input: &str) -> String {
    let mut out = String::new();

    if !stm.trim().is_empty() {
        out.push_str("CONTEXT (from head's short-term memory):\n");
        out.push_str(stm.trim());
        out.push_str("\n\n");
    }

    out.push_str("TASK\n");
    out.push_str("goal: ");
    out.push_str(goal.trim());
    out.push('\n');
    if !input.trim().is_empty() {
        out.push_str("input:\n");
        out.push_str(input.trim());
        out.push('\n');
    }
    out
}
