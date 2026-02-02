use std::sync::Arc;

use crate::history::Store;
use crate::llm::{ChatMessage, Role};
use crate::runtime::SnapshotManager;
use crate::runtime::{atomic_write_file_0600, read_optional_file, workspace_head_memory};
use std::path::PathBuf;

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
        }
    }

    pub fn with_autist(mut self, autist: AutistMode) -> Self {
        self.autist = autist;
        self
    }
}

pub struct HandBundleBuilder {
    store: Arc<Store>,
    workspace_root: PathBuf,
    system: String,
    snapshot: Arc<SnapshotManager>,
    autist_adhd: String,
    autist_neurotypical: String,
    autist_autist: String,
    autist_fullretard: String,
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
        let autist_adhd = include_str!("../traits/autist/adhd.md");
        let autist_neurotypical = include_str!("../traits/autist/neurotypical.md");
        let autist_autist = include_str!("../traits/autist/autist.md");
        let autist_fullretard = include_str!("../traits/autist/full-retard.md");
        Self {
            store,
            workspace_root,
            system: system.to_string(),
            snapshot,
            autist_adhd: autist_adhd.to_string(),
            autist_neurotypical: autist_neurotypical.to_string(),
            autist_autist: autist_autist.to_string(),
            autist_fullretard: autist_fullretard.to_string(),
        }
    }

    fn autist_prompt(&self, autist: &AutistMode) -> Option<&str> {
        match autist {
            AutistMode::None => None,
            AutistMode::Adhd => Some(&self.autist_adhd),
            AutistMode::Neurotypical => Some(&self.autist_neurotypical),
            AutistMode::Autist => Some(&self.autist_autist),
            AutistMode::FullRetard => Some(&self.autist_fullretard),
        }
    }

    pub fn build(&self, cfg: &HandBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        let snap = self.snapshot.get();

        // System message: commandments + tools + environment + autist prompt (if any)
        let autist_prompt = self
            .autist_prompt(&cfg.autist)
            .map(|p| format!("\n\n{}", p))
            .unwrap_or_default();
        let system_content = format!(
            "{}\n\n{}\n\n{}\n\n{}{}",
            self.system,
            snap.commandments_md.trim(),
            snap.hand_tools_md.trim(),
            snap.environment_md.trim(),
            autist_prompt
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_initial_messages() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let builder = HandBundleBuilder::new(store, std::env::current_dir().unwrap());

        let cfg = HandBundleConfig::new("t-1", "head-0", "list files", "");
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].role, Role::System));
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("You are a hand")
        );
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("## Tools")
        );
        assert!(matches!(messages[1].role, Role::User));
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("goal: list files")
        );
    }

    #[test]
    fn builds_conversation_from_history() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        // Log some hand execs
        store
            .log_hand_exec(
                "t-2",
                "hand-1",
                0,
                "bash",
                "ls",
                "file1\nfile2",
                true,
                10,
                "<exec tool=\"bash\">ls</exec>",
            )
            .unwrap();
        store
            .log_hand_exec(
                "t-2",
                "hand-1",
                1,
                "read",
                "file1",
                "contents",
                true,
                5,
                "<exec tool=\"read\">file1</exec>",
            )
            .unwrap();

        let builder = HandBundleBuilder::new(store, std::env::current_dir().unwrap());
        let cfg = HandBundleConfig::new("t-2", "head-1", "read files", "");
        let messages = builder.build(&cfg);

        // System + Initial + 2*(Assistant + User)
        assert_eq!(messages.len(), 6);

        assert!(matches!(messages[0].role, Role::System));
        assert!(matches!(messages[1].role, Role::User));
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("goal: read files")
        );

        assert!(matches!(messages[2].role, Role::Assistant));
        assert!(
            messages[2]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("<exec tool=\"bash\">ls</exec>")
        );

        assert!(matches!(messages[3].role, Role::User));
        assert!(
            messages[3]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("[Tool bash completed]")
        );
        assert!(
            messages[3]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("file1\nfile2")
        );

        assert!(matches!(messages[4].role, Role::Assistant));
        assert!(
            messages[4]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("<exec tool=\"read\">file1</exec>")
        );

        assert!(matches!(messages[5].role, Role::User));
        assert!(
            messages[5]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("[Tool read completed]")
        );
        assert!(
            messages[5]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("contents")
        );
    }

    #[test]
    fn includes_stm_in_initial_prompt() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        // Set STM for the head
        store
            .set_head_stm(
                "head-2",
                "Working on refactoring auth module.\nUser prefers functional style.",
            )
            .unwrap();

        let builder = HandBundleBuilder::new(store, std::env::current_dir().unwrap());
        let cfg = HandBundleConfig::new("t-3", "head-2", "update login function", "");
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);

        // Initial prompt should contain STM context
        let initial = messages[1].content.as_deref().unwrap_or("");
        assert!(initial.contains("CONTEXT"));
        assert!(initial.contains("refactoring auth module"));
        assert!(initial.contains("functional style"));
        assert!(initial.contains("goal: update login function"));
    }

    #[test]
    fn skips_empty_stm() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let builder = HandBundleBuilder::new(store, std::env::current_dir().unwrap());

        let cfg = HandBundleConfig::new("t-4", "head-3", "list files", "");
        let messages = builder.build(&cfg);

        // Initial prompt should NOT contain CONTEXT section when STM is empty
        let initial = messages[1].content.as_deref().unwrap_or("");
        assert!(!initial.contains("CONTEXT"));
        assert!(initial.contains("goal: list files"));
    }
}
