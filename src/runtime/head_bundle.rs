use std::sync::Arc;

use crate::runtime::SnapshotManager;
use crate::bus::{Message, MessageData, MessageOp, Origin, Scope, TaskMsg};
use crate::history::Store;
use crate::llm::{ChatMessage, Role};
use crate::runtime::{atomic_write_file_0600, read_optional_file, sandbox_mind_memory_from_workspace_root};
use std::path::PathBuf;
use uuid::Uuid;
use std::collections::BTreeMap;

use super::TarsDials;

/// Generation mode controls Head communication style.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GenerationMode {
    /// No generation style - default behavior
    #[default]
    None,
    /// Boomer - verbose, over-explains, writes docs
    Boomer,
    /// GenX - minimal, cynical, gets it done
    GenX,
    /// Millennial - over-communicates, seeks validation
    Millennial,
    /// GenZ - terse, ships fast, no ceremony
    GenZ,
    /// Alpha - chaotic digital native
    Alpha,
}

impl GenerationMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "boomer" => Some(Self::Boomer),
            "genx" => Some(Self::GenX),
            "millennial" => Some(Self::Millennial),
            "genz" => Some(Self::GenZ),
            "alpha" => Some(Self::Alpha),
            "" | "none" => Some(Self::None),
            _ => None,
        }
    }
}

pub struct HeadBundleConfig {
    pub head_id: String,
    pub scopes: Vec<Scope>,
    pub max_messages_per_scope: usize,
    pub context_budget_tokens: Option<u32>,
    pub generation: GenerationMode,
    pub tars: TarsDials,
}

impl HeadBundleConfig {
    pub fn new(head_id: impl Into<String>, scopes: Vec<Scope>) -> Self {
        Self {
            head_id: head_id.into(),
            scopes,
            max_messages_per_scope: 100,
            context_budget_tokens: None,
            generation: GenerationMode::None,
            tars: TarsDials::default(),
        }
    }

    pub fn with_context_budget_tokens(mut self, budget: Option<u32>) -> Self {
        self.context_budget_tokens = budget;
        self
    }

    pub fn with_generation(mut self, generation: GenerationMode) -> Self {
        self.generation = generation;
        self
    }

    pub fn with_tars(mut self, tars: TarsDials) -> Self {
        self.tars = tars;
        self
    }
}

pub struct HeadBundleBuilder {
    store: Arc<Store>,
    workspace_root: PathBuf,
    identity: String,
    context: String,
    behavior: String,
    snapshot: Arc<SnapshotManager>,
    gen_boomer: String,
    gen_genx: String,
    gen_millennial: String,
    gen_genz: String,
    gen_alpha: String,
}

impl HeadBundleBuilder {
    pub fn new(store: Arc<Store>, workspace_root: PathBuf) -> Self {
        let snapshot = SnapshotManager::new(workspace_root.clone());
        Self::new_with_snapshot(store, workspace_root, snapshot)
    }

    pub fn new_with_snapshot(store: Arc<Store>, workspace_root: PathBuf, snapshot: Arc<SnapshotManager>) -> Self {
        let identity = include_str!("head_identity.md");
        let context = include_str!("head_context.md");
        let behavior = include_str!("head_behavior.md");
        let gen_boomer = include_str!("../traits/generation/boomer.md");
        let gen_genx = include_str!("../traits/generation/genx.md");
        let gen_millennial = include_str!("../traits/generation/millennial.md");
        let gen_genz = include_str!("../traits/generation/genz.md");
        let gen_alpha = include_str!("../traits/generation/alpha.md");
        Self {
            store,
            workspace_root,
            identity: identity.to_string(),
            context: context.to_string(),
            behavior: behavior.to_string(),
            snapshot,
            gen_boomer: gen_boomer.to_string(),
            gen_genx: gen_genx.to_string(),
            gen_millennial: gen_millennial.to_string(),
            gen_genz: gen_genz.to_string(),
            gen_alpha: gen_alpha.to_string(),
        }
    }

    fn generation_prompt(&self, generation: &GenerationMode) -> Option<&str> {
        match generation {
            GenerationMode::None => None,
            GenerationMode::Boomer => Some(&self.gen_boomer),
            GenerationMode::GenX => Some(&self.gen_genx),
            GenerationMode::Millennial => Some(&self.gen_millennial),
            GenerationMode::GenZ => Some(&self.gen_genz),
            GenerationMode::Alpha => Some(&self.gen_alpha),
        }
    }

    pub fn build(&self, cfg: &HeadBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        let snap = self.snapshot.get();

        // System message: identity + commandments + tools + LTM (if any) + generation prompt (if any)
        let ltm = self.load_global_ltm();
        let generation_prompt = self.generation_prompt(&cfg.generation)
            .map(|p| format!("\n\n{}", p))
            .unwrap_or_default();

        let external_tools_md = {
            let mut by_name: BTreeMap<String, String> = BTreeMap::new();
            for scope in &cfg.scopes {
                let scope_str = scope.to_string();
                if let Ok(rows) = self.store.list_tool_summaries(&scope_str, "external") {
                    for r in rows {
                        by_name.entry(r.name).or_insert(r.summary);
                    }
                }
            }

            if by_name.is_empty() {
                String::new()
            } else {
                let mut lines = String::new();
                for (name, summary) in by_name {
                    lines.push_str(&format!("- `client__{}`: {}\n", name, summary));
                }
                format!("\n\n## External Tools (client)\n\n{}", lines.trim_end())
            }
        };

        // Build system prompt in order:
        // 1. Identity (role intro)
        // 2. Commandments + Prohibitions
        // 3. Context (Memory, Escalation, Workspaces)
        // 4. Tools (Head, Hand, External)
        // 5. Behavior (Truncation, Communication, Local Dev Mode)
        // 6. Environment
        // 7. Long-Term Memory (if any)
        // 8. Generation prompt (if any)
        // 9. TARS dials (if any)
        let ltm_section = if ltm.is_empty() {
            String::new()
        } else {
            format!("\n\n## Long-Term Memory\n\n{}", ltm)
        };

        let tars_section = if cfg.tars.is_empty() {
            String::new()
        } else {
            format!("\n\n{}", cfg.tars.render())
        };

        let system_content = format!(
            "{}\n\n{}\n\n{}\n\n## Head Tools\n\n{}\n\n## Hand Tools (via tasks_create)\n\n{}{}\n\n{}\n\n{}{}{}{}",
            self.identity.trim(),
            snap.commandments_md.trim(),
            self.context.trim(),
            snap.head_tools_md.trim(),
            snap.hand_tools_md.trim(),
            external_tools_md,
            self.behavior.trim(),
            snap.environment_md.trim(),
            ltm_section,
            generation_prompt,
            tars_section,
        );
        let system_tokens = estimate_tokens(&system_content);
        messages.push(ChatMessage::new(Role::System, system_content));

        let boundary_ms = self
            .store
            .last_event_ts_ms("main", "conclave_done", 200)
            .unwrap_or(0);

        // Gather and sort all messages from all scopes by timestamp
        let mut all_messages: Vec<Message> = Vec::new();
        for scope in &cfg.scopes {
            let scope_str = scope.to_string();
            let scope_messages = self
                .store
                .recent(&scope_str, cfg.max_messages_per_scope)
                .unwrap_or_default();
            all_messages.extend(scope_messages);
        }

        if boundary_ms > 0 {
            all_messages.retain(|m| {
                m.timestamp
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| (d.as_millis() as i64) > boundary_ms)
                    .unwrap_or(true)
            });
        }

        // Sort by timestamp (oldest first for conversation order)
        all_messages.sort_by_key(|m| m.timestamp);

        // Convert to chat messages with appropriate roles
        let mut history: Vec<(Role, String, Origin)> = Vec::new();
        for msg in all_messages {
            let role = self.message_role(&msg, &cfg.head_id);
            let is_self = msg.origin == Origin::Head && msg.sender == cfg.head_id;
            let content = render_message(&msg, is_self);

            if !content.is_empty() {
                history.push((role, content, msg.origin));
            }
        }

        if let Some(budget) = cfg.context_budget_tokens {
            let mut total = system_tokens;
            for (_, c, _) in &history {
                total += estimate_tokens(c);
            }

            // Keep the last human message if present.
            let mut last_human_idx = history
                .iter()
                .rposition(|(_, _, o)| *o == Origin::Human);

            while total > budget as usize && history.len() > 1 {
                if last_human_idx == Some(0) {
                    break;
                }
                let removed = history.remove(0);
                total = total.saturating_sub(estimate_tokens(&removed.1));
                last_human_idx = last_human_idx.map(|i| i.saturating_sub(1));
            }
        }

        for (role, content, _) in history {
            messages.push(ChatMessage::new(role, content));
        }

        messages
    }

    fn load_global_ltm(&self) -> String {
        let Some(path) = sandbox_mind_memory_from_workspace_root(&self.workspace_root) else {
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

    fn message_role(&self, msg: &Message, head_id: &str) -> Role {
        // Assistant = this head speaking
        // User = everyone else (humans, other heads, system, hands)
        if msg.origin == Origin::Head && msg.sender == head_id {
            Role::Assistant
        } else {
            Role::User
        }
    }
}

fn render_message(msg: &Message, is_self: bool) -> String {
    // Skip prefix for the head's own messages to avoid teaching it to echo "[Abbot]"
    let prefix = if is_self {
        String::new()
    } else if let Some(reply_to) = msg.reply_to {
        format!("[{}↩{}] ", msg.sender, short_uuid(reply_to))
    } else {
        format!("[{}] ", msg.sender)
    };

    match (&msg.op, &msg.data) {
        (MessageOp::Chat, MessageData::Text(t)) => {
            if is_self {
                t.clone()
            } else {
                format!("{}{}", prefix, t)
            }
        }
        (MessageOp::Task, MessageData::Task(task_msg)) => render_task_message(&prefix, task_msg),
        _ => String::new(),
    }
}

fn short_uuid(id: Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

fn render_task_message(prefix: &str, task: &TaskMsg) -> String {
    match task {
        TaskMsg::Request { task_id, goal, .. } => {
            format!("{}task {} requested: {}", prefix, task_id, goal)
        }
        TaskMsg::Assigned {
            task_id, hand_id, ..
        } => {
            format!("{}task {} assigned to {}", prefix, task_id, hand_id)
        }
        TaskMsg::Progress { task_id, note, .. } => {
            format!("{}task {} progress: {}", prefix, task_id, note)
        }
        TaskMsg::ToolCall {
            task_id, tool, args, ..
        } => {
            let preview: String = args.to_string().chars().take(160).collect();
            format!(
                "{}task {} tool_call: {} {}",
                prefix,
                task_id,
                tool,
                preview
            )
        }
        TaskMsg::ToolDone {
            task_id,
            tool,
            ok,
            duration_ms,
            error_code,
            ..
        } => {
            if *ok {
                format!(
                    "{}task {} tool_done: {} ok ({}ms)",
                    prefix, task_id, tool, duration_ms
                )
            } else if let Some(code) = error_code {
                format!(
                    "{}task {} tool_done: {} error={} ({}ms)",
                    prefix, task_id, tool, code, duration_ms
                )
            } else {
                format!(
                    "{}task {} tool_done: {} failed ({}ms)",
                    prefix, task_id, tool, duration_ms
                )
            }
        }
        TaskMsg::Echo {
            task_id,
            tool,
            content,
            ..
        } => {
            format!("{}task {} echo from {}: {}", prefix, task_id, tool, content)
        }
        TaskMsg::Result {
            task_id,
            ok,
            summary,
            ..
        } => {
            let status = if *ok { "completed" } else { "failed" };
            format!("{}task {} {}: {}", prefix, task_id, status, summary)
        }
    }
}

fn estimate_tokens(s: &str) -> usize {
    // Conservative-ish approximation: ~4 chars/token for English.
    // This is only used for trimming, not for exact budgeting.
    (s.chars().count() + 3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::bus::{Hub, Origin, Scope, respond};
    use crate::runtime::RuntimeBus;

    #[tokio::test]
    async fn builds_conversation_with_roles() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        let hub = Arc::new(RwLock::new(Hub::new()));
        let bus = RuntimeBus::new(hub, store.clone());
        bus.create_scope(Scope::from("#general")).await;

        // Human says something
        bus.publish(respond::chat("alice", "#general", "hello monk").with_origin(Origin::Human))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Head (Monk) responds
        bus.publish(respond::chat("Monk", "#general", "hello alice").with_origin(Origin::Head))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Human asks question
        bus.publish(respond::chat("alice", "#general", "can you help?").with_origin(Origin::Human))
            .await;

        let builder = HeadBundleBuilder::new(store, std::env::current_dir().unwrap());
        let cfg = HeadBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        // System + 3 conversation messages
        assert_eq!(messages.len(), 4);

        assert!(matches!(messages[0].role, Role::System));
        assert!(messages[0].content.as_deref().unwrap_or("").contains("Head"));

        // Human message -> User role
        assert!(matches!(messages[1].role, Role::User));
        assert!(messages[1].content.as_deref().unwrap_or("").contains("alice"));
        assert!(messages[1].content.as_deref().unwrap_or("").contains("hello monk"));

        // Head message -> Assistant role (own messages don't include sender prefix)
        assert!(matches!(messages[2].role, Role::Assistant));
        assert!(messages[2].content.as_deref().unwrap_or("").contains("hello alice"));

        // Human message -> User role
        assert!(matches!(messages[3].role, Role::User));
        assert!(messages[3].content.as_deref().unwrap_or("").contains("can you help"));
    }

    #[tokio::test]
    async fn includes_task_messages() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        let hub = Arc::new(RwLock::new(Hub::new()));
        let bus = RuntimeBus::new(hub, store.clone());
        bus.create_scope(Scope::from("#general")).await;

        bus.publish(
            respond::task_result("hand-1", "#general", "t-1", "hand-1", true, "done")
                .with_origin(Origin::Hand),
        )
        .await;

        let builder = HeadBundleBuilder::new(store, std::env::current_dir().unwrap());
        let cfg = HeadBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[1].role, Role::User));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("task t-1 completed"));
    }
}
