use std::path::PathBuf;
use std::sync::Arc;

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope};
use crate::history::Store;
use crate::llm::{ChatMessage, Role};

/// Wake mode determines what context to inject on Mind startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WakeMode {
    /// Normal tick - no special boot context
    #[default]
    Normal,
    /// First-time startup - no prior history exists
    Init,
    /// Reboot - prior history exists, resuming operation
    Boot,
}

pub struct MindBundleConfig {
    pub head_id: String,
    pub scopes: Vec<Scope>,
    pub max_messages: usize,
    pub wake_mode: WakeMode,
    pub workspace: Option<PathBuf>,
}

impl MindBundleConfig {
    pub fn new(head_id: impl Into<String>, scopes: Vec<Scope>) -> Self {
        Self {
            head_id: head_id.into(),
            scopes,
            max_messages: 50,
            wake_mode: WakeMode::Normal,
            workspace: None,
        }
    }

    pub fn with_wake_mode(mut self, wake_mode: WakeMode) -> Self {
        self.wake_mode = wake_mode;
        self
    }

    pub fn with_workspace(mut self, workspace: PathBuf) -> Self {
        self.workspace = Some(workspace);
        self
    }
}

pub struct MindBundleBuilder {
    store: Arc<Store>,
    system: String,
    grammar: String,
    init_prompt: String,
    boot_prompt: String,
}

impl MindBundleBuilder {
    pub fn new(store: Arc<Store>) -> Self {
        let system = include_str!("mind_system.md");
        let grammar = include_str!("mind_grammar.md");
        let init_prompt = include_str!("init.md");
        let boot_prompt = include_str!("boot.md");
        Self {
            store,
            system: system.to_string(),
            grammar: grammar.to_string(),
            init_prompt: init_prompt.to_string(),
            boot_prompt: boot_prompt.to_string(),
        }
    }

    pub fn build(&self, cfg: &MindBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System message: identity + grammar + optional wake prompt
        let wake_prompt = match cfg.wake_mode {
            WakeMode::Init => format!("\n\n{}", self.init_prompt),
            WakeMode::Boot => format!("\n\n{}", self.boot_prompt),
            WakeMode::Normal => String::new(),
        };
        let system_content = format!("{}\n\n{}{}", self.system, self.grammar, wake_prompt);
        messages.push(ChatMessage::new(Role::System, system_content));

        // User message: LTM + recent head activity
        let user_content = self.build_user_context(cfg);
        messages.push(ChatMessage::new(Role::User, user_content));

        messages
    }

    fn build_user_context(&self, cfg: &MindBundleConfig) -> String {
        let mut sections = Vec::new();

        // Workspace context (environment, files, git, AGENTS.md, README.md)
        if let Some(workspace) = &cfg.workspace {
            sections.push(Self::build_workspace_context(workspace));
        }

        // Current LTM
        let ltm = self
            .store
            .get_head_ltm(&cfg.head_id)
            .unwrap_or_default();

        sections.push(format!(
            "## Current Long-Term Memory\n\n{}",
            if ltm.is_empty() {
                "(empty - no memories yet)".to_string()
            } else {
                ltm
            }
        ));

        // Recent head activity
        let activity = self.gather_recent_activity(cfg);
        sections.push(format!(
            "## Recent Head Activity\n\n{}",
            if activity.is_empty() {
                "(no recent activity)".to_string()
            } else {
                activity
            }
        ));

        // On boot, include additional system state
        if cfg.wake_mode == WakeMode::Boot {
            sections.push(self.build_boot_context());
        }

        sections.join("\n\n")
    }

    fn build_boot_context(&self) -> String {
        let mut sections = Vec::new();

        // System stats
        let wants_count = self.store.count_wants().unwrap_or(0);
        let recent = self.store.recent_any(100).unwrap_or_default();
        let chat_count = recent.iter().filter(|m| m.op == MessageOp::Chat).count();
        let task_count = recent.iter().filter(|m| m.op == MessageOp::Task).count();
        let need_count = recent.iter().filter(|m| m.op == MessageOp::Need).count();
        let error_count = recent.iter().filter(|m| m.op == MessageOp::Error).count();

        sections.push(format!(
            "## System State\n\n\
             - Wants pool: {} items\n\
             - Recent messages (last 100): {} chat, {} task, {} need, {} error",
            wants_count, chat_count, task_count, need_count, error_count
        ));

        // Wants pool summary (top 10)
        if let Ok(wants) = self.store.list_wants(10) {
            if !wants.is_empty() {
                let wants_list: Vec<String> = wants
                    .iter()
                    .map(|w| format!("- [{}] {}", w.priority, w.want))
                    .collect();
                sections.push(format!(
                    "## Wants Pool (top {})\n\n{}",
                    wants.len(),
                    wants_list.join("\n")
                ));
            }
        }

        // Recent needs (check for incomplete work)
        let recent_needs: Vec<_> = recent
            .iter()
            .filter(|m| m.op == MessageOp::Need)
            .take(10)
            .collect();

        if !recent_needs.is_empty() {
            let needs_list: Vec<String> = recent_needs
                .iter()
                .filter_map(|m| {
                    if let MessageData::Need(need_msg) = &m.data {
                        match need_msg {
                            crate::bus::NeedMsg::Request { need_id, need, .. } => {
                                Some(format!("- [{}] {}", &need_id[..8.min(need_id.len())], need))
                            }
                            crate::bus::NeedMsg::Acknowledged { need_id, head_id } => {
                                Some(format!("- [{}] acknowledged by {}", &need_id[..8.min(need_id.len())], head_id))
                            }
                            crate::bus::NeedMsg::Fulfilled { need_id, .. } => {
                                Some(format!("- [{}] (fulfilled)", &need_id[..8.min(need_id.len())]))
                            }
                            crate::bus::NeedMsg::Expired { need_id, reason } => {
                                Some(format!("- [{}] expired: {}", &need_id[..8.min(need_id.len())], reason))
                            }
                        }
                    } else {
                        None
                    }
                })
                .collect();

            if !needs_list.is_empty() {
                sections.push(format!(
                    "## Recent Needs\n\n{}",
                    needs_list.join("\n")
                ));
            }
        }

        // Recent goals/tasks (check for incomplete work)
        let recent_tasks: Vec<_> = recent
            .iter()
            .filter(|m| m.op == MessageOp::Task)
            .take(10)
            .collect();

        if !recent_tasks.is_empty() {
            let tasks_list: Vec<String> = recent_tasks
                .iter()
                .filter_map(|m| {
                    if let MessageData::Task(task_msg) = &m.data {
                        match task_msg {
                            crate::bus::TaskMsg::Request { task_id, goal, .. } => {
                                Some(format!("- [{}] requested: {}", &task_id[..8.min(task_id.len())], goal))
                            }
                            crate::bus::TaskMsg::Result { task_id, ok, summary, .. } => {
                                let status = if *ok { "completed" } else { "failed" };
                                Some(format!("- [{}] {}: {}", &task_id[..8.min(task_id.len())], status, summary))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                })
                .collect();

            if !tasks_list.is_empty() {
                sections.push(format!(
                    "## Recent Goals\n\n{}",
                    tasks_list.join("\n")
                ));
            }
        }

        sections.join("\n\n")
    }

    fn build_workspace_context(workspace: &PathBuf) -> String {
        let mut sections = Vec::new();

        // Environment info
        let platform = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let now = chrono::Local::now();
        sections.push(format!(
            "## Environment\n\n\
             - Platform: {} ({})\n\
             - Local time: {}\n\
             - Workspace: {}",
            platform,
            arch,
            now.format("%Y-%m-%d %H:%M:%S %Z"),
            workspace.display()
        ));

        // List top-level files
        if let Ok(entries) = std::fs::read_dir(workspace) {
            let mut files: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    !name.starts_with('.')
                })
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let is_dir = e.path().is_dir();
                    if is_dir {
                        format!("{}/", name)
                    } else {
                        name
                    }
                })
                .collect();
            files.sort();

            if !files.is_empty() {
                sections.push(format!(
                    "## Workspace Files\n\n```\n{}\n```",
                    files.join("\n")
                ));
            } else {
                sections.push("## Workspace Files\n\n(empty)".to_string());
            }
        }

        // Check if git repo and get recent commits
        let git_dir = workspace.join(".git");
        if git_dir.exists() {
            if let Ok(output) = std::process::Command::new("git")
                .args(["log", "--oneline", "-10"])
                .current_dir(workspace)
                .output()
            {
                if output.status.success() {
                    let commits = String::from_utf8_lossy(&output.stdout);
                    let commits = commits.trim();
                    if !commits.is_empty() {
                        sections.push(format!(
                            "## Recent Git Commits\n\n```\n{}\n```",
                            commits
                        ));
                    }
                }
            }

            // Get current branch
            if let Ok(output) = std::process::Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(workspace)
                .output()
            {
                if output.status.success() {
                    let branch = String::from_utf8_lossy(&output.stdout);
                    let branch = branch.trim();
                    if !branch.is_empty() {
                        sections.push(format!("## Git Branch\n\n`{}`", branch));
                    }
                }
            }
        }

        // Read AGENTS.md if present
        let agents_path = workspace.join("AGENTS.md");
        if agents_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&agents_path) {
                let content = content.trim();
                if !content.is_empty() {
                    sections.push(format!(
                        "## AGENTS.md\n\n{}",
                        Self::truncate_chars(content, 4000)
                    ));
                }
            }
        }

        // Read README.md if present
        let readme_path = workspace.join("README.md");
        if readme_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&readme_path) {
                let content = content.trim();
                if !content.is_empty() {
                    sections.push(format!(
                        "## README.md\n\n{}",
                        Self::truncate_chars(content, 4000)
                    ));
                }
            }
        }

        sections.join("\n\n")
    }

    fn truncate_chars(s: &str, max_chars: usize) -> String {
        if s.chars().count() <= max_chars {
            s.to_string()
        } else {
            let truncated: String = s.chars().take(max_chars).collect();
            format!("{}...\n\n(truncated)", truncated)
        }
    }

    fn gather_recent_activity(&self, cfg: &MindBundleConfig) -> String {
        let mut all_messages: Vec<Message> = Vec::new();

        for scope in &cfg.scopes {
            let scope_str = scope.to_string();
            let scope_messages = self
                .store
                .recent(&scope_str, cfg.max_messages)
                .unwrap_or_default();
            all_messages.extend(scope_messages);
        }

        // Sort by timestamp (oldest first)
        all_messages.sort_by_key(|m| m.timestamp);

        // Render each message
        all_messages
            .iter()
            .filter_map(|msg| render_activity_message(msg))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn render_activity_message(msg: &Message) -> Option<String> {
    let origin_label = match msg.origin {
        Origin::Human => "human",
        Origin::Head => "head",
        Origin::Hand => "hand",
        Origin::System => "system",
    };

    match (&msg.op, &msg.data) {
        (MessageOp::Chat, MessageData::Text(t)) => {
            Some(format!("[{} {}] {}", origin_label, msg.sender, t))
        }
        (MessageOp::Task, MessageData::Task(task_msg)) => {
            use crate::bus::TaskMsg;
            match task_msg {
                TaskMsg::Request { goal, .. } => {
                    Some(format!("[{} {}] delegated: {}", origin_label, msg.sender, goal))
                }
                TaskMsg::Result { ok, summary, .. } => {
                    let status = if *ok { "completed" } else { "failed" };
                    Some(format!(
                        "[{} {}] task {}: {}",
                        origin_label, msg.sender, status, summary
                    ))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::bus::{Hub, Origin, Scope, respond};
    use crate::runtime::RuntimeBus;

    #[tokio::test]
    async fn builds_context_with_ltm_and_activity() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        // Set some LTM
        store.set_head_ltm("Monk", "Curious about: Rust patterns.").unwrap();

        let hub = Arc::new(RwLock::new(Hub::new()));
        let bus = RuntimeBus::new(hub, store.clone());
        bus.create_scope(Scope::from("#general")).await;

        // Add some activity
        bus.publish(
            respond::chat("alice", "#general", "Can you help with this?")
                .with_origin(Origin::Human),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        bus.publish(
            respond::chat("Monk", "#general", "Sure, I'll look into it.")
                .with_origin(Origin::Head),
        )
        .await;

        let builder = MindBundleBuilder::new(store);
        let cfg = MindBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);

        // System message
        assert!(matches!(messages[0].role, Role::System));
        assert!(messages[0]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("Mind"));
        assert!(messages[0]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("update_ltm"));

        // User message with LTM and activity
        assert!(matches!(messages[1].role, Role::User));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("Long-Term Memory"));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("Rust patterns"));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("Recent Head Activity"));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("alice"));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("Monk"));
    }

    #[tokio::test]
    async fn handles_empty_ltm() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        let builder = MindBundleBuilder::new(store);
        let cfg = MindBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("empty - no memories yet"));
    }
}
