use std::sync::Arc;
use std::path::PathBuf;

use crate::agent_tools::{describe_tools, mind_tool_specs};
use crate::bus::{Message, MessageData, MessageOp, Origin, Scope};
use crate::history::Store;
use crate::llm::{ChatMessage, Role};
use crate::runtime::{
    atomic_write_file_0600,
    read_optional_file,
    sandbox_mind_memory_from_workspace_root,
    sandbox_mind_self_from_workspace_root,
};

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

/// Fever mode controls Mind creativity/initiative level.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FeverMode {
    /// No fever - normal caretaker mode
    #[default]
    None,
    /// Mild - be more exploratory
    Mild,
    /// Hot - take initiative, less hedging
    Hot,
    /// Delirium - fuck it, we ball
    Delirium,
    /// Meth - vibrating at incomprehensible frequencies
    Meth,
}

impl FeverMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "mild" => Some(Self::Mild),
            "hot" => Some(Self::Hot),
            "delirium" => Some(Self::Delirium),
            "meth" => Some(Self::Meth),
            "" | "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn prompt_file(&self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Mild => Some("mild.md"),
            Self::Hot => Some("hot.md"),
            Self::Delirium => Some("delirium.md"),
            Self::Meth => Some("meth.md"),
        }
    }
}

/// Room type for mind meetings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoomType {
    /// Autonomy - operational retro, what's next (default, triggered on 5 min idle)
    #[default]
    Autonomy,
    /// Conclave - strategic identity/memory meeting (rare, triggered on 1 hour idle)
    Conclave,
}

pub struct MindBundleConfig {
    pub head_id: String,
    pub scopes: Vec<Scope>,
    pub max_messages: usize,
    pub wake_mode: WakeMode,
    pub workspace: Option<PathBuf>,
    pub fever: FeverMode,
    pub room_type: RoomType,
}

impl MindBundleConfig {
    pub fn new(head_id: impl Into<String>, scopes: Vec<Scope>) -> Self {
        Self {
            head_id: head_id.into(),
            scopes,
            max_messages: 50,
            wake_mode: WakeMode::Normal,
            workspace: None,
            fever: FeverMode::None,
            room_type: RoomType::Conclave,
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

    pub fn with_fever(mut self, fever: FeverMode) -> Self {
        self.fever = fever;
        self
    }

    pub fn with_room_type(mut self, room_type: RoomType) -> Self {
        self.room_type = room_type;
        self
    }
}

pub struct MindBundleBuilder {
    store: Arc<Store>,
    system: String,
    commandments: String,
    tools: String,
    init_prompt: String,
    boot_prompt: String,
    fever_mild: String,
    fever_hot: String,
    fever_delirium: String,
    fever_meth: String,
}

impl MindBundleBuilder {
    pub fn new(store: Arc<Store>) -> Self {
        let system = include_str!("mind_system.md");
        let commandments = include_str!("commandments.md");
        let tools = describe_tools(&mind_tool_specs());
        let init_prompt = include_str!("init.md");
        let boot_prompt = include_str!("boot.md");
        let fever_mild = include_str!("../traits/fever/mild.md");
        let fever_hot = include_str!("../traits/fever/hot.md");
        let fever_delirium = include_str!("../traits/fever/delirium.md");
        let fever_meth = include_str!("../traits/fever/meth.md");
        Self {
            store,
            system: system.to_string(),
            commandments: commandments.to_string(),
            tools,
            init_prompt: init_prompt.to_string(),
            boot_prompt: boot_prompt.to_string(),
            fever_mild: fever_mild.to_string(),
            fever_hot: fever_hot.to_string(),
            fever_delirium: fever_delirium.to_string(),
            fever_meth: fever_meth.to_string(),
        }
    }

    fn fever_prompt(&self, fever: &FeverMode) -> Option<&str> {
        match fever {
            FeverMode::None => None,
            FeverMode::Mild => Some(&self.fever_mild),
            FeverMode::Hot => Some(&self.fever_hot),
            FeverMode::Delirium => Some(&self.fever_delirium),
            FeverMode::Meth => Some(&self.fever_meth),
        }
    }

    pub fn build(&self, cfg: &MindBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System message: identity + commandments + tools + optional wake prompt + optional fever
        let wake_prompt = match cfg.wake_mode {
            WakeMode::Init => format!("\n\n{}", self.init_prompt),
            WakeMode::Boot => format!("\n\n{}", self.boot_prompt),
            WakeMode::Normal => String::new(),
        };
        let fever_prompt = self.fever_prompt(&cfg.fever)
            .map(|p| format!("\n\n{}", p))
            .unwrap_or_default();
        let system_content = format!("{}\n\n{}\n\n{}{}{}", self.system, self.commandments, self.tools, wake_prompt, fever_prompt);
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

         // Current Self (collective identity)
         let self_identity = self.load_global_self(cfg);

        sections.push(format!(
            "## Current Self (Collective Identity)\n\n{}",
            if self_identity.is_empty() {
                "(empty - no identity defined yet)".to_string()
            } else {
                self_identity
            }
        ));

         // Current LTM
         let ltm = self.load_global_ltm(cfg);

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

        // For autonomy meetings, include GitHub issues/PRs if gh plugin is enabled
        if cfg.room_type == RoomType::Autonomy {
            if let Some(workspace) = &cfg.workspace {
                if Self::is_gh_plugin_enabled(workspace) {
                    if let Some(github_context) = Self::fetch_github_context(workspace) {
                        sections.push(github_context);
                    }
                }
            }
        }

        sections.join("\n\n")
    }

    fn load_global_self(&self, cfg: &MindBundleConfig) -> String {
        let Some(workspace_root) = cfg.workspace.as_ref() else {
            return String::new();
        };

        let Some(path) = sandbox_mind_self_from_workspace_root(workspace_root) else {
            return String::new();
        };

        if let Ok(Some(content)) = read_optional_file(&path) {
            return content;
        }

        // One-time migration from legacy DB location.
        let legacy = self.store.get_conclave_self().unwrap_or_default();
        if !legacy.trim().is_empty() {
            let _ = atomic_write_file_0600(&path, legacy.trim());
            return legacy;
        }

        String::new()
    }

    fn load_global_ltm(&self, cfg: &MindBundleConfig) -> String {
        let Some(workspace_root) = cfg.workspace.as_ref() else {
            return String::new();
        };

        let Some(path) = sandbox_mind_memory_from_workspace_root(workspace_root) else {
            return String::new();
        };

        if let Ok(Some(content)) = read_optional_file(&path) {
            return content;
        }

        // One-time migration from legacy DB location.
        let legacy = self.store.get_head_ltm("conclave").unwrap_or_default();
        if !legacy.trim().is_empty() {
            let _ = atomic_write_file_0600(&path, legacy.trim());
            return legacy;
        }

        String::new()
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
                    "## Recent Tasks\n\n{}",
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

    fn is_gh_plugin_enabled(workspace: &PathBuf) -> bool {
        // Check if gh plugin is enabled by reading plugins.toml
        let sandbox_dir = match crate::runtime::sandbox_dir_from_workspace_root(workspace) {
            Some(d) => d,
            None => return false,
        };

        let plugins_toml = sandbox_dir.join("plugins.toml");
        if !plugins_toml.exists() {
            return false;
        }

        let content = match std::fs::read_to_string(&plugins_toml) {
            Ok(c) => c,
            Err(_) => return false,
        };

        // Simple check - look for "gh" in the enabled list
        // Format: enabled = ["gh", ...]
        content.contains("\"gh\"")
    }

    fn fetch_github_context(workspace: &PathBuf) -> Option<String> {
        let mut sections = Vec::new();

        // Fetch open issues (limit 100, most recent first)
        if let Some(issues) = Self::fetch_gh_issues(workspace) {
            if !issues.is_empty() {
                sections.push(format!("## GitHub Issues (open)\n\n{}", issues));
            }
        }

        // Fetch open PRs (limit 50)
        if let Some(prs) = Self::fetch_gh_prs(workspace) {
            if !prs.is_empty() {
                sections.push(format!("## GitHub Pull Requests (open)\n\n{}", prs));
            }
        }

        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n\n"))
        }
    }

    fn fetch_gh_issues(workspace: &PathBuf) -> Option<String> {
        let output = std::process::Command::new("gh")
            .args([
                "issue", "list",
                "--limit", "100",
                "--state", "open",
                "--json", "number,title,labels",
            ])
            .current_dir(workspace)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let json_str = String::from_utf8_lossy(&output.stdout);
        let issues: Vec<serde_json::Value> = serde_json::from_str(&json_str).ok()?;

        if issues.is_empty() {
            return Some("(no open issues)".to_string());
        }

        let lines: Vec<String> = issues
            .iter()
            .filter_map(|issue| {
                let number = issue.get("number")?.as_i64()?;
                let title = issue.get("title")?.as_str()?;
                let labels: Vec<String> = issue
                    .get("labels")
                    .and_then(|l| l.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|l| l.get("name").and_then(|n| n.as_str()))
                            .map(|s| s.to_string())
                            .collect()
                    })
                    .unwrap_or_default();

                let label_str = if labels.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", labels.join(", "))
                };

                Some(format!("#{} {}{}", number, title, label_str))
            })
            .collect();

        Some(lines.join("\n"))
    }

    fn fetch_gh_prs(workspace: &PathBuf) -> Option<String> {
        let output = std::process::Command::new("gh")
            .args([
                "pr", "list",
                "--limit", "50",
                "--state", "open",
                "--json", "number,title,author,isDraft",
            ])
            .current_dir(workspace)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let json_str = String::from_utf8_lossy(&output.stdout);
        let prs: Vec<serde_json::Value> = serde_json::from_str(&json_str).ok()?;

        if prs.is_empty() {
            return Some("(no open PRs)".to_string());
        }

        let lines: Vec<String> = prs
            .iter()
            .filter_map(|pr| {
                let number = pr.get("number")?.as_i64()?;
                let title = pr.get("title")?.as_str()?;
                let author = pr
                    .get("author")
                    .and_then(|a| a.get("login"))
                    .and_then(|l| l.as_str())
                    .unwrap_or("unknown");
                let is_draft = pr.get("isDraft").and_then(|d| d.as_bool()).unwrap_or(false);

                let status = if is_draft { " [draft]" } else { "" };

                Some(format!("#{} {} by @{}{}", number, title, author, status))
            })
            .collect();

        Some(lines.join("\n"))
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

        let base = std::env::temp_dir().join(format!(
            "abbot-mind-bundle-{}",
            uuid::Uuid::new_v4().to_string()
        ));
        let workspace_root = base.join("root");
        let mind_dir = base.join("mind");
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&mind_dir).unwrap();
        std::fs::write(mind_dir.join("memory.md"), "Curious about: Rust patterns.").unwrap();

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
        let cfg = MindBundleConfig::new("Monk", vec![Scope::from("#general")])
            .with_workspace(workspace_root);
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
