use std::path::PathBuf;
use std::sync::Arc;

use crate::syscalls::dispatch::{describe_tools, mind_catalog};
use crate::history::Store;
use crate::kernel::{ConversationItem, FrameSelectArgs};
use crate::llm::{ChatMessage, Role};
use crate::runtime::Kernel;
use crate::runtime::{
    atomic_write_file_0600, read_optional_file, workspace_dir_from_root, workspace_mind_memory,
    workspace_mind_self,
};
use crate::scope::Scope;
use crate::runtime::{SystemBundler, TarsDials};
use crate::runtime::SystemSlot;
use crate::runtime::{AutistMode, GenerationMode};

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

pub use super::types::RoomType;

pub struct RoomBundleConfig {
    pub head_id: String,
    pub scopes: Vec<Scope>,
    pub max_messages: usize,
    pub wake_mode: WakeMode,
    pub workspace: Option<PathBuf>,
    pub frames_db_path: Option<PathBuf>,
    pub fever: FeverMode,
    pub filter: crate::runtime::FilterMode,
    pub poverty: crate::runtime::PovertyMode,
    pub room_type: RoomType,
}

impl RoomBundleConfig {
    pub fn new(head_id: impl Into<String>, scopes: Vec<Scope>) -> Self {
        Self {
            head_id: head_id.into(),
            scopes,
            max_messages: 50,
            wake_mode: WakeMode::Normal,
            workspace: None,
            frames_db_path: None,
            fever: FeverMode::None,
            filter: crate::runtime::FilterMode::None,
            poverty: crate::runtime::PovertyMode::None,
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

    pub fn with_frames_db_path(mut self, path: PathBuf) -> Self {
        self.frames_db_path = Some(path);
        self
    }

    pub fn with_fever(mut self, fever: FeverMode) -> Self {
        self.fever = fever;
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

    pub fn with_room_type(mut self, room_type: RoomType) -> Self {
        self.room_type = room_type;
        self
    }
}

pub struct RoomBundleBuilder {
    store: Arc<Store>,
    system: String,
    init_prompt: String,
    boot_prompt: String,
}

impl RoomBundleBuilder {
    pub fn new(store: Arc<Store>) -> Self {
        let system = include_str!("../mind_system.md");
        let init_prompt = include_str!("../init.md");
        let boot_prompt = include_str!("../boot.md");
        Self {
            store,
            system: system.to_string(),
            init_prompt: init_prompt.to_string(),
            boot_prompt: boot_prompt.to_string(),
        }
    }

    pub fn build(&self, cfg: &RoomBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System message: identity + commandments + tools + optional wake prompt + optional traits
        let wake_prompt = match cfg.wake_mode {
            WakeMode::Init => self.init_prompt.trim().to_string(),
            WakeMode::Boot => self.boot_prompt.trim().to_string(),
            WakeMode::Normal => String::new(),
        };
        let tools = describe_tools(&mind_catalog());
        let workspace_root = cfg.workspace.as_deref();

        let mut bundler = SystemBundler::new()
            .with_layer(SystemSlot::Core, self.system.clone())
            .with_commandments()
            .with_layer(SystemSlot::Context, wake_prompt)
            .with_tools_section(SystemSlot::ToolsPrimary, "Tools", &tools)
            .with_traits_and_tars(
                &TarsDials::default(),
                &cfg.fever,
                &GenerationMode::None,
                &AutistMode::None,
                &cfg.filter,
                &cfg.poverty,
            );

        if let Some(ws) = workspace_root {
            bundler = bundler.with_environment_and_network(ws);
        }

        let system_content = bundler.build();
        messages.push(ChatMessage::new(Role::System, system_content));

        // User message: LTM + recent head activity
        let user_content = self.build_user_context(cfg);
        messages.push(ChatMessage::new(Role::User, user_content));

        messages
    }

    fn build_user_context(&self, cfg: &RoomBundleConfig) -> String {
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

    fn load_global_self(&self, cfg: &RoomBundleConfig) -> String {
        let Some(workspace_root) = cfg.workspace.as_ref() else {
            return String::new();
        };

        let path = workspace_mind_self(workspace_root);

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

    fn load_global_ltm(&self, cfg: &RoomBundleConfig) -> String {
        let Some(workspace_root) = cfg.workspace.as_ref() else {
            return String::new();
        };

        let path = workspace_mind_memory(workspace_root);

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
        let (chat_count, task_count, need_count, error_count) = self.recent_frame_counts(100);

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
        if let Some(needs_section) = self.recent_needs_section() {
            sections.push(needs_section);
        }

        // Recent goals/tasks (check for incomplete work)
        if let Some(tasks_section) = self.recent_tasks_section() {
            sections.push(tasks_section);
        }

        sections.join("\n\n")
    }

    fn build_workspace_context(workspace: &PathBuf) -> String {
        let mut sections = Vec::new();

        // List top-level files
        if let Ok(entries) = std::fs::read_dir(workspace) {
            let mut files = Vec::new();
            for entry in entries {
                let Ok(entry) = entry else { continue };
                let Some(name) = format_dir_entry(&entry) else {
                    continue;
                };
                files.push(name);
            }
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
                        sections.push(format!("## Recent Git Commits\n\n```\n{}\n```", commits));
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
        // Check if gh plugin is enabled by reading plugins.toml.
        let workspace_dir = workspace_dir_from_root(workspace);

        let plugins_toml = workspace_dir.join("plugins.toml");
        if !plugins_toml.exists() {
            return false;
        }

        let content = match std::fs::read_to_string(&plugins_toml) {
            Ok(c) => c,
            Err(_) => return false,
        };

        let v: toml::Value = match toml::from_str(&content) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let Some(table) = v.as_table() else {
            return false;
        };

        // Legacy format: enabled = ["gh", ...]
        if let Some(enabled) = table.get("enabled").and_then(|v| v.as_array()) {
            for item in enabled {
                if item.as_str() == Some("gh") {
                    return true;
                }
            }
        }

        // Current format: [gh] enabled = true
        table
            .get("gh")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
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
                "issue",
                "list",
                "--limit",
                "100",
                "--state",
                "open",
                "--json",
                "number,title,labels",
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

        let mut lines = Vec::new();
        for issue in &issues {
            let Some(line) = format_github_issue(issue) else {
                continue;
            };
            lines.push(line);
        }

        Some(lines.join("\n"))
    }

    fn fetch_gh_prs(workspace: &PathBuf) -> Option<String> {
        let output = std::process::Command::new("gh")
            .args([
                "pr",
                "list",
                "--limit",
                "50",
                "--state",
                "open",
                "--json",
                "number,title,author,isDraft",
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

        let mut lines = Vec::new();
        for pr in &prs {
            let Some(line) = format_github_pr(pr) else {
                continue;
            };
            lines.push(line);
        }

        Some(lines.join("\n"))
    }

    fn gather_recent_activity(&self, cfg: &RoomBundleConfig) -> String {
        let mut all_items: Vec<ConversationItem> = self.fetch_conversation_items(cfg);
        all_items.sort_by_key(|m| (m.ts_ms, m.seq));

        all_items
            .iter()
            .filter_map(render_activity_message)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn fetch_conversation_items(&self, cfg: &RoomBundleConfig) -> Vec<ConversationItem> {
        let db_path: Option<PathBuf> = cfg.frames_db_path.clone().or_else(|| {
            let k = Kernel::get()?;
            let store = k.frames()?;
            Some(store.db_path().to_path_buf())
        });
        let Some(db_path) = db_path else {
            return Vec::new();
        };

        let mut all_items: Vec<ConversationItem> = Vec::new();
        for scope in &cfg.scopes {
            let mut args = FrameSelectArgs::default();
            // Don't rely on the indexed scope column; filter via frame JSON.
            // Kernel is a global singleton in tests, and older logs may not have scope indexed.
            args.query = Some(format!("\"scope\":\"{}\"", scope));
            args.limit = Some(cfg.max_messages as u64);
            // Pull the most recent items per scope; we sort chronologically after combining.
            args.order = Some("desc".to_string());

            if let Ok((items, _)) =
                crate::kernel::frame_select::select_conversation(&db_path, &args)
            {
                all_items.extend(items);
            }
        }

        all_items
    }

    fn fetch_recent_conversation(&self, limit: usize) -> Vec<ConversationItem> {
        let Some(k) = Kernel::get() else {
            return Vec::new();
        };
        let Some(store) = k.frames() else {
            return Vec::new();
        };

        let mut args = FrameSelectArgs::default();
        args.limit = Some(limit as u64);
        args.order = Some("desc".to_string());
        match crate::kernel::frame_select::select_conversation(store.db_path(), &args) {
            Ok((items, _)) => items,
            Err(_) => Vec::new(),
        }
    }

    fn recent_needs_section(&self) -> Option<String> {
        let items = self.fetch_recent_conversation(200);
        let mut needs_list = Vec::new();
        for item in items.iter().filter(|i| i.kind == "need").take(10) {
            if let Some(line) = format_need_item(item) {
                needs_list.push(line);
            }
        }
        if needs_list.is_empty() {
            None
        } else {
            Some(format!("## Recent Needs\n\n{}", needs_list.join("\n")))
        }
    }

    fn recent_tasks_section(&self) -> Option<String> {
        let items = self.fetch_recent_conversation(200);
        let mut tasks_list = Vec::new();
        for item in items.iter().filter(|i| i.kind == "task").take(10) {
            if let Some(line) = format_task_item(item) {
                tasks_list.push(line);
            }
        }
        if tasks_list.is_empty() {
            None
        } else {
            Some(format!("## Recent Tasks\n\n{}", tasks_list.join("\n")))
        }
    }

    fn recent_frame_counts(&self, limit: usize) -> (usize, usize, usize, usize) {
        let Some(k) = Kernel::get() else {
            return (0, 0, 0, 0);
        };
        let Some(store) = k.frames() else {
            return (0, 0, 0, 0);
        };

        let conn = match rusqlite::Connection::open(store.db_path()) {
            Ok(c) => c,
            Err(_) => return (0, 0, 0, 0),
        };

        let mut stmt = match conn
            .prepare("SELECT op, frame_json FROM frames ORDER BY seq DESC LIMIT ?1")
        {
            Ok(s) => s,
            Err(_) => return (0, 0, 0, 0),
        };

        let mut rows = match stmt.query([limit as i64]) {
            Ok(r) => r,
            Err(_) => return (0, 0, 0, 0),
        };

        let mut chat_count = 0;
        let mut task_count = 0;
        let mut need_count = 0;
        let mut error_count = 0;

        while let Ok(Some(row)) = rows.next() {
            let op: String = row.get(0).unwrap_or_default();
            let frame_json: String = row.get(1).unwrap_or_else(|_| "{}".to_string());

            if op == "Error" {
                error_count += 1;
                continue;
            }

            let Ok(frame) = serde_json::from_str::<crate::kernel::Frame>(&frame_json) else {
                continue;
            };

            if let Some(name) = frame.name.as_deref() {
                if name.starts_with("task:") {
                    task_count += 1;
                    continue;
                }
                if name.starts_with("need:") {
                    need_count += 1;
                    continue;
                }
            }

            if frame.op == crate::kernel::FrameOp::Event {
                if let Some(data) = frame.data.as_ref() {
                    if let Some(kind) = data.get("kind").and_then(|v| v.as_str()) {
                        if kind.starts_with("chat:") {
                            chat_count += 1;
                        }
                    }
                }
            }
        }

        (chat_count, task_count, need_count, error_count)
    }
}

fn render_activity_message(item: &ConversationItem) -> Option<String> {
    let sender = item.sender.as_deref().unwrap_or("unknown");
    let origin_label = activity_origin_label(sender, &item.role);
    if item.content.trim().is_empty() {
        None
    } else {
        Some(format!("[{} {}] {}", origin_label, sender, item.content))
    }
}

fn format_dir_entry(entry: &std::fs::DirEntry) -> Option<String> {
    let name = entry.file_name().to_string_lossy().to_string();
    if name.starts_with('.') {
        return None;
    }

    if entry.path().is_dir() {
        Some(format!("{}/", name))
    } else {
        Some(name)
    }
}

fn format_github_issue(issue: &serde_json::Value) -> Option<String> {
    let number = issue.get("number")?.as_i64()?;
    let title = issue.get("title")?.as_str()?;
    let labels = extract_label_names(issue);

    let label_str = if labels.is_empty() {
        String::new()
    } else {
        format!(" [{}]", labels.join(", "))
    };

    Some(format!("#{} {}{}", number, title, label_str))
}

fn extract_label_names(issue: &serde_json::Value) -> Vec<String> {
    let Some(arr) = issue.get("labels").and_then(|l| l.as_array()) else {
        return Vec::new();
    };

    let mut names = Vec::new();
    for label in arr {
        let Some(name) = label.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        names.push(name.to_string());
    }
    names
}

fn format_github_pr(pr: &serde_json::Value) -> Option<String> {
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
}

fn format_need_item(item: &ConversationItem) -> Option<String> {
    let need = item.need.as_ref()?;
    let id = &need.need_id;
    let short = &id[..8.min(id.len())];
    match need.status.as_str() {
        "requested" => Some(format!(
            "- [{}] {}",
            short,
            need.need.as_deref().unwrap_or("")
        )),
        "fulfilled" => Some(format!("- [{}] (fulfilled)", short)),
        other => Some(format!("- [{}] {}", short, other)),
    }
}

fn format_task_item(item: &ConversationItem) -> Option<String> {
    let task = item.task.as_ref()?;
    let id = &task.task_id;
    let short = &id[..8.min(id.len())];
    match task.status.as_str() {
        "requested" => Some(format!(
            "- [{}] requested: {}",
            short,
            task.goal.as_deref().unwrap_or("")
        )),
        "completed" | "failed" => Some(format!(
            "- [{}] {}: {}",
            short,
            task.status,
            task.summary.as_deref().unwrap_or("")
        )),
        other => Some(format!("- [{}] {}", short, other)),
    }
}

fn activity_origin_label(sender: &str, role: &str) -> &'static str {
    if sender.starts_with("human/") {
        "human"
    } else if sender.starts_with("head/") {
        "head"
    } else if sender.starts_with("hand/") {
        "hand"
    } else if sender.starts_with("system/") {
        "system"
    } else if role == "assistant" {
        "head"
    } else if role == "system" {
        "system"
    } else {
        "human"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::kernel::{FrameStore, Frame};
    use crate::runtime::Kernel;
    use crate::scope::Scope;
    use uuid::Uuid;

    async fn ensure_kernel_with_audit() -> Arc<Kernel> {
        if let Some(k) = Kernel::get() {
            if k.frames().is_some() {
                return k;
            }
        }

        let root =
            std::env::temp_dir().join(format!("abbot-mind-bundle-{}", Uuid::new_v4().to_string()));
        std::fs::create_dir_all(&root).unwrap();

        let k = Kernel::get().unwrap_or_else(|| Kernel::init(&root));
        if k.frames().is_none() {
            let frames_db = root.join("frames.db");
            let store = FrameStore::open(&frames_db).unwrap();
            k.set_frames(store).await;
        }
        k
    }

    #[tokio::test]
    async fn builds_context_with_ltm_and_activity() {
        let history_store = Arc::new(Store::open(":memory:").unwrap());

        let base =
            std::env::temp_dir().join(format!("abbot-mind-bundle-{}", Uuid::new_v4().to_string()));
        let workspace_root = base.join("root");
        let mind_dir = base.join("mind");
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&mind_dir).unwrap();
        std::fs::write(mind_dir.join("memory.md"), "Curious about: Rust patterns.").unwrap();

        // Use a unique scope to avoid cross-test interference (Kernel is a global singleton).
        let scope = format!("#mind-bundle-{}", Uuid::new_v4());

        // Use a private frames.db to avoid cross-test interference from the global Kernel singleton.
        let frames_db = base.join("frames.db");
        let frame_store = FrameStore::open(&frames_db).unwrap();

        // Append frames directly to the frame store.
        frame_store
            .append(
                Frame::req(
                    "frames:append",
                    serde_json::json!({
                        "kind": "chat:user",
                        "scope": scope.clone(),
                        "data": {"content": "Can you help with this?"}
                    }),
                )
                .with_actor("human/alice"),
            )
            .await;
        frame_store
            .append(
                Frame::req(
                    "frames:append",
                    serde_json::json!({
                        "kind": "chat:head",
                        "scope": scope.clone(),
                        "data": {"sender": "Monk", "content": "Sure, I'll look into it."}
                    }),
                )
                .with_actor("head/Monk"),
            )
            .await;

        // Wait until both frames are visible in frames.db.
        let mut args = crate::kernel::frame_select::FrameSelectArgs::default();
        args.query = Some(format!("\"scope\":\"{}\"", scope.as_str()));
        args.limit = Some(200);
        args.order = Some("desc".to_string());

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok((items, _)) = crate::kernel::frame_select::select_conversation(frame_store.db_path(), &args) {
                    let found_alice = items
                        .iter()
                        .any(|i| i.sender.as_deref() == Some("human/alice"));
                    let found_monk = items
                        .iter()
                        .any(|i| i.sender.as_deref() == Some("Monk") || i.sender.as_deref() == Some("head/Monk"));
                    if found_alice && found_monk {
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("frame store did not flush appended frames in time");

        let builder = RoomBundleBuilder::new(history_store);
        let cfg = RoomBundleConfig::new("Monk", vec![Scope::from(scope.as_str())])
            .with_workspace(workspace_root)
            .with_frames_db_path(frames_db);
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);

        // System message
        assert!(matches!(messages[0].role, Role::System));
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("Mind")
        );
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("mind__ltm_update")
        );

        // User message with LTM and activity
        assert!(matches!(messages[1].role, Role::User));
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("Long-Term Memory")
        );
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("Rust patterns")
        );
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("Recent Head Activity")
        );
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("alice")
        );
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("Monk")
        );
    }

    #[tokio::test]
    async fn handles_empty_ltm() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let _ = ensure_kernel_with_audit().await;

        let builder = RoomBundleBuilder::new(store);
        let cfg = RoomBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("empty - no memories yet")
        );
    }
}
