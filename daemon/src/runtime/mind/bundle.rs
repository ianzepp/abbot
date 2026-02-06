//! Mind Loop Bundle Builder
//!
//! Assembles context for each mind loop wake cycle. Builds a system message
//! (identity, commandments, tools, environment, tone) and a user message
//! (workspace context, self/LTM, system state, seen/new activity split).

use std::path::PathBuf;
use std::sync::Arc;

use crate::history::Store;
use crate::kernel::{ConversationItem, FrameSelectArgs};
use crate::hal::llm::{ChatMessage, Role};
use crate::runtime::Kernel;
use crate::runtime::{
    read_optional_file, workspace_mind_memory, workspace_mind_self,
};
use crate::runtime::{SystemBundler, TarsDials};
use crate::runtime::SystemSlot;
use crate::runtime::room::bundle::RoomBundleBuilder;
use crate::syscalls::dispatch::{describe_tools, mind_loop_catalog};

/// Configuration for a single mind loop wake cycle.
pub struct MindLoopBundleConfig {
    pub channel: String,
    pub workspace: PathBuf,
    pub max_context_items: usize,
    pub last_wake_ts: Option<i64>,
}

impl MindLoopBundleConfig {
    pub fn new(channel: impl Into<String>, workspace: PathBuf) -> Self {
        Self {
            channel: channel.into(),
            workspace,
            max_context_items: 100,
            last_wake_ts: None,
        }
    }

    pub fn with_last_wake_ts(mut self, ts: Option<i64>) -> Self {
        self.last_wake_ts = ts;
        self
    }

    pub fn with_max_context_items(mut self, n: usize) -> Self {
        self.max_context_items = n;
        self
    }
}

pub struct MindLoopBundleBuilder {
    store: Arc<Store>,
}

impl MindLoopBundleBuilder {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn build(&self, cfg: &MindLoopBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System message
        let tools = describe_tools(&mind_loop_catalog());

        let bundler = SystemBundler::new()
            .with_layer(
                SystemSlot::Core,
                include_str!("mind_loop_system.md").to_string(),
            )
            .with_commandments()
            .with_tools_section(SystemSlot::ToolsPrimary, "Tools", &tools)
            .with_environment_and_network(&cfg.workspace)
            .with_traits_and_tars(
                &TarsDials::default(),
                &crate::runtime::FeverMode::None,
                &crate::runtime::GenerationMode::None,
                &crate::runtime::AutistMode::None,
                &crate::runtime::FilterMode::None,
                &crate::runtime::PovertyMode::None,
            );

        let system_content = bundler.build();
        messages.push(ChatMessage::new(Role::System, system_content));

        // User message
        let user_content = self.build_user_context(cfg);
        messages.push(ChatMessage::new(Role::User, user_content));

        messages
    }

    fn build_user_context(&self, cfg: &MindLoopBundleConfig) -> String {
        let mut sections = Vec::new();

        // Workspace context (files, git, AGENTS.md)
        sections.push(RoomBundleBuilder::build_workspace_context(&cfg.workspace));

        // Current Self
        let self_identity = self.load_self(&cfg.workspace);
        sections.push(format!(
            "## Current Self\n\n{}",
            if self_identity.is_empty() {
                "(empty - no identity defined yet)".to_string()
            } else {
                self_identity
            }
        ));

        // Current LTM
        let ltm = self.load_ltm(&cfg.workspace);
        sections.push(format!(
            "## Current Long-Term Memory\n\n{}",
            if ltm.is_empty() {
                "(empty - no memories yet)".to_string()
            } else {
                ltm
            }
        ));

        // System state (queue counts)
        sections.push(self.build_system_state());

        // Activity sections (seen/new split)
        let (seen, new) = self.gather_activity(cfg);
        if let Some(seen_section) = seen {
            sections.push(seen_section);
        }
        sections.push(new);

        sections.join("\n\n")
    }

    fn load_self(&self, workspace: &PathBuf) -> String {
        let path = workspace_mind_self(workspace);
        read_optional_file(&path)
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn load_ltm(&self, workspace: &PathBuf) -> String {
        let path = workspace_mind_memory(workspace);
        read_optional_file(&path)
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn build_system_state(&self) -> String {
        let mut lines = Vec::new();
        lines.push("## System State".to_string());

        // Queue counts via kernel
        if let Some(k) = Kernel::get() {
            let rt = tokio::runtime::Handle::current();
            let (need_q, need_active) = rt.block_on(k.needs().counts());
            let (task_q, task_running, task_done) = rt.block_on(k.tasks().counts());

            lines.push(format!(
                "- Need queue: {} queued, {} active",
                need_q, need_active
            ));
            lines.push(format!(
                "- Task queue: {} queued, {} running, {} done",
                task_q, task_running, task_done
            ));
        }

        // Wants count
        let wants_count = self.store.count_wants().unwrap_or(0);
        lines.push(format!("- Wants pool: {} items", wants_count));

        lines.join("\n")
    }

    /// Gather activity split into "previously reviewed" and "new since last wake".
    ///
    /// On first wake (last_wake_ts == None), returns no "previously reviewed"
    /// section and all recent frames as "new".
    fn gather_activity(
        &self,
        cfg: &MindLoopBundleConfig,
    ) -> (Option<String>, String) {
        let db_path = self.resolve_frames_db();
        let Some(db_path) = db_path else {
            return (None, "## New Activity\n\n(no frame store available)".to_string());
        };

        let scope_query = format!("\"scope\":\"#{}\"", cfg.channel);

        match cfg.last_wake_ts {
            Some(last_ts) => {
                // Previously reviewed: frames before last_wake_ts
                let seen_items = self.select_frames(
                    &db_path,
                    &scope_query,
                    None,
                    Some(last_ts),
                    50,
                );
                let seen_section = if seen_items.is_empty() {
                    None
                } else {
                    let rendered = Self::render_items(&seen_items);
                    Some(format!("## Previously Reviewed\n\n{}", rendered))
                };

                // New since last wake
                let new_items = self.select_frames(
                    &db_path,
                    &scope_query,
                    Some(last_ts),
                    None,
                    cfg.max_context_items as u64,
                );
                let new_section = if new_items.is_empty() {
                    "## New Since Last Wake\n\n(no new activity)".to_string()
                } else {
                    let rendered = Self::render_items(&new_items);
                    format!("## New Since Last Wake\n\n{}", rendered)
                };

                (seen_section, new_section)
            }
            None => {
                // First wake: all recent frames as "new"
                let items = self.select_frames(
                    &db_path,
                    &scope_query,
                    None,
                    None,
                    cfg.max_context_items as u64,
                );
                let section = if items.is_empty() {
                    "## Recent Activity\n\n(no recent activity)".to_string()
                } else {
                    let rendered = Self::render_items(&items);
                    format!("## Recent Activity\n\n{}", rendered)
                };
                (None, section)
            }
        }
    }

    fn resolve_frames_db(&self) -> Option<PathBuf> {
        let k = Kernel::get()?;
        let store = k.frames()?;
        Some(store.db_path().to_path_buf())
    }

    fn select_frames(
        &self,
        db_path: &PathBuf,
        scope_query: &str,
        since_ts_ms: Option<i64>,
        until_ts_ms: Option<i64>,
        limit: u64,
    ) -> Vec<ConversationItem> {
        let mut args = FrameSelectArgs::default();
        args.query = Some(scope_query.to_string());
        args.since_ts_ms = since_ts_ms;
        args.until_ts_ms = until_ts_ms;
        args.limit = Some(limit);
        args.order = Some("desc".to_string());

        match crate::kernel::frame_select::select_conversation(db_path, &args) {
            Ok((mut items, _)) => {
                // Reverse to chronological order (select returns desc)
                items.reverse();
                items
            }
            Err(_) => Vec::new(),
        }
    }

    fn render_items(items: &[ConversationItem]) -> String {
        items
            .iter()
            .filter_map(|item| {
                let sender = item.sender.as_deref().unwrap_or("unknown");
                let origin = activity_origin_label(sender, &item.role);
                if item.content.trim().is_empty() {
                    None
                } else {
                    Some(format!("[{} {}] {}", origin, sender, item.content))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn activity_origin_label(sender: &str, role: &str) -> &'static str {
    if sender.starts_with("human/") {
        "human"
    } else if sender.starts_with("head/") {
        "head"
    } else if sender.starts_with("hand/") {
        "hand"
    } else if sender.starts_with("mind/") {
        "mind"
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
