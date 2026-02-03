use std::sync::Arc;

use crate::history::Store;
use crate::kernel::{ConversationItem, LogSelectArgs};
use crate::llm::{ChatMessage, Role};
use crate::runtime::Kernel;
use crate::runtime::RuntimeSnapshot;
use crate::runtime::SnapshotManager;
use crate::runtime::{atomic_write_file_0600, read_optional_file, workspace_mind_memory};
use crate::scope::Scope;
use std::collections::BTreeMap;
use std::path::PathBuf;

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
        let snapshot = SnapshotManager::new(workspace_root.clone(), Some(store.clone()));
        Self::new_with_snapshot(store, workspace_root, snapshot)
    }

    pub fn new_with_snapshot(
        store: Arc<Store>,
        workspace_root: PathBuf,
        snapshot: Arc<SnapshotManager>,
    ) -> Self {
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

        let ltm = self.load_global_ltm();
        let generation_prompt = self.generation_prompt(&cfg.generation);

        let system_layers = vec![
            self.get_layer_0_identity(),
            self.get_layer_1_commandments(&snap),
            self.get_layer_2_context(),
            self.get_layer_3_head_tools(&snap),
            self.get_layer_4_hand_tools(&snap),
            self.get_layer_5_external_tools(&cfg.scopes),
            self.get_layer_6_behavior(),
            self.get_layer_7_environment(&snap, &cfg.scopes),
            self.get_layer_8_long_term_memory(&ltm),
            self.get_layer_9_generation_and_tars(generation_prompt, &cfg.tars),
        ];

        let system_content = system_layers
            .into_iter()
            .filter(|layer| !layer.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut system_tokens = estimate_tokens(&system_content);
        messages.push(ChatMessage::new(Role::System, system_content));

        if let Some(user_prompt) = self.load_user_prompt(&cfg.scopes) {
            let prompt_tokens = estimate_tokens(&user_prompt);
            system_tokens += prompt_tokens;
            messages.push(ChatMessage::new(Role::System, user_prompt));
        }

        // Gather and sort all messages from all scopes by timestamp
        let mut all_messages: Vec<ConversationItem> = self.fetch_conversation_items(cfg);

        // Sort by timestamp (oldest first for conversation order)
        all_messages.sort_by_key(|m| (m.ts_ms, m.seq));

        // Convert to chat messages with appropriate roles
        let mut history: Vec<(Role, String, bool)> = Vec::new();
        for msg in all_messages {
            let is_self =
                msg.sender.as_deref() == Some(cfg.head_id.as_str()) && msg.role == "assistant";
            let role = if is_self { Role::Assistant } else { Role::User };
            let content = render_message(&msg, is_self);
            let is_human = msg
                .sender
                .as_deref()
                .map(|s| s.starts_with("human/"))
                .unwrap_or(false);

            if !content.is_empty() {
                history.push((role, content, is_human));
            }
        }

        if let Some(budget) = cfg.context_budget_tokens {
            let mut total = system_tokens;
            for (_, c, _) in &history {
                total += estimate_tokens(c);
            }

            // Keep the last human message if present.
            let mut last_human_idx = history.iter().rposition(|(_, _, is_human)| *is_human);

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

    fn fetch_conversation_items(&self, cfg: &HeadBundleConfig) -> Vec<ConversationItem> {
        let Some(k) = Kernel::get() else {
            return Vec::new();
        };
        let Some(audit) = k.audit() else {
            return Vec::new();
        };

        let mut all_items: Vec<ConversationItem> = Vec::new();
        for scope in &cfg.scopes {
            let mut args = LogSelectArgs::default();
            args.scope = Some(scope.to_string());
            args.limit = Some(cfg.max_messages_per_scope as u64);
            args.order = Some("asc".to_string());

            if let Ok((items, _)) =
                crate::kernel::log_select::select_conversation(audit.db_path(), &args)
            {
                all_items.extend(items);
            }
        }

        all_items
    }

    fn load_global_ltm(&self) -> String {
        let path = workspace_mind_memory(&self.workspace_root);

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

    /// Layer 0: Establishes the head's identity and mission parameters.
    ///
    /// WHY: Re-centers the LLM on its core role before any tactical details,
    /// ensuring downstream layers inherit the right persona.
    fn get_layer_0_identity(&self) -> String {
        self.identity.trim().to_string()
    }

    /// Layer 1: Lists the commandments/prohibitions governing head behavior.
    ///
    /// WHY: Keeps policy constraints loaded ahead of tools so every action is
    /// evaluated through the organization's safety lens.
    fn get_layer_1_commandments(&self, snap: &RuntimeSnapshot) -> String {
        snap.commandments_md.trim().to_string()
    }

    /// Layer 2: Documents STM, escalation, and workspace routing context.
    ///
    /// WHY: Reminds the head how to manage memory boundaries and when to
    /// escalate work instead of improvising.
    fn get_layer_2_context(&self) -> String {
        self.context.trim().to_string()
    }

    /// Layer 3: Describes head tools and playbooks.
    ///
    /// WHY: Tool fluency precedes action; surfacing capabilities early reduces
    /// hallucinated operations.
    fn get_layer_3_head_tools(&self, snap: &RuntimeSnapshot) -> String {
        format!("## Head Tools\n\n{}", snap.head_tools_md.trim())
    }

    /// Layer 4: Explains hand tooling and how to delegate via tasks.
    ///
    /// WHY: Reinforces the delegation contract so heads offload work instead of
    /// burning context on file IO or exploration.
    fn get_layer_4_hand_tools(&self, snap: &RuntimeSnapshot) -> String {
        format!(
            "## Hand Tools (via head__task_create)\n\n{}",
            snap.hand_tools_md.trim()
        )
    }

    /// Layer 5: Summarizes user-provided external tools (if any).
    ///
    /// WHY: Keeps untrusted client capabilities isolated in one section so the
    /// head can consciously opt into them.
    fn get_layer_5_external_tools(&self, scopes: &[Scope]) -> String {
        let mut by_name: BTreeMap<String, String> = BTreeMap::new();
        for scope in scopes {
            let scope_str = scope.to_string();
            if let Ok(rows) = self.store.list_tool_summaries(&scope_str, "external") {
                for r in rows {
                    by_name.entry(r.name).or_insert(r.summary);
                }
            }
        }

        if by_name.is_empty() {
            return String::new();
        }

        let mut lines = String::new();
        for (name, summary) in by_name {
            lines.push_str(&format!("- `user__{}`: {}\n", name, summary));
        }
        format!("## External Tools (user)\n\n{}", lines.trim_end())
    }

    /// Layer 6: Conveys behavioral guardrails (delegation, mutation, truncation).
    ///
    /// WHY: Serves as a checklist before executing tools, mirroring the
    /// formatter guidance to focus on WHY not WHAT.
    fn get_layer_6_behavior(&self) -> String {
        self.behavior.trim().to_string()
    }

    /// Layer 7: Combines environment facts, network binding, and session env.
    ///
    /// WHY: The head needs a single place to reason about host vs client
    /// topology to avoid leaking or assuming incorrect paths.
    fn get_layer_7_environment(&self, snap: &RuntimeSnapshot, scopes: &[Scope]) -> String {
        let mut out = snap.environment_md.trim().to_string();
        let mut env_blocks = Vec::new();
        for scope in scopes {
            let scope_str = scope.to_string();
            if let Ok(Some(env)) = self.store.get_session_env(&scope_str) {
                let trimmed = env.trim().to_string();
                if !trimmed.is_empty() {
                    env_blocks.push((scope_str, trimmed));
                }
            }
        }

        if !env_blocks.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            if env_blocks.len() == 1 {
                out.push_str("## Client Environment\n\n");
                out.push_str(&env_blocks[0].1);
            } else {
                out.push_str("## Client Environment\n");
                for (scope, env) in env_blocks {
                    out.push_str(&format!("\n### {}\n\n{}\n", scope, env));
                }
                while out.ends_with('\n') {
                    if out.ends_with("\n\n") {
                        out.pop();
                        out.pop();
                    } else {
                        break;
                    }
                }
            }
        }

        out.trim_end().to_string()
    }

    /// Layer 8: Surfaces long-term memory, when present.
    ///
    /// WHY: Keeps strategic memories anchored near environment details so heads
    /// can connect workspace state with prior lessons.
    fn get_layer_8_long_term_memory(&self, ltm: &str) -> String {
        if ltm.trim().is_empty() {
            String::new()
        } else {
            format!("## Long-Term Memory\n\n{}", ltm.trim())
        }
    }

    /// Layer 9: Applies generation persona overlays and TARS dials.
    ///
    /// WHY: Persona tuning is optional; grouping it with TARS keeps all tone
    /// modifiers in one slot for predictable ordering.
    fn get_layer_9_generation_and_tars(
        &self,
        generation_prompt: Option<&str>,
        tars: &TarsDials,
    ) -> String {
        let mut out = String::new();
        if let Some(prompt) = generation_prompt {
            out.push_str(prompt.trim());
        }
        let tars_block = tars.render();
        let tars_trimmed = tars_block.trim();
        if !tars_trimmed.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(tars_trimmed);
        }
        out
    }

    fn load_user_prompt(&self, scopes: &[Scope]) -> Option<String> {
        for scope in scopes {
            if let Ok(Some(prompt)) = self.store.get_scope_user_prompt(scope.as_str()) {
                let trimmed = prompt.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
        None
    }
}

fn render_message(msg: &ConversationItem, is_self: bool) -> String {
    // Skip prefix for the head's own messages to avoid teaching it to echo "[Abbot]"
    let prefix = if is_self {
        String::new()
    } else if let (Some(sender), Some(reply_to)) = (&msg.sender, &msg.reply_to) {
        format!("[{}↩{}] ", sender, short_id(reply_to))
    } else {
        msg.sender
            .as_ref()
            .map(|s| format!("[{}] ", s))
            .unwrap_or_default()
    };

    match msg.kind.as_str() {
        "chat" => {
            if is_self {
                msg.content.clone()
            } else {
                format!("{}{}", prefix, msg.content)
            }
        }
        "task" | "need" => format!("{}{}", prefix, msg.content),
        _ => String::new(),
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn estimate_tokens(s: &str) -> usize {
    // Conservative-ish approximation: ~4 chars/token for English.
    // This is only used for trimming, not for exact budgeting.
    (s.chars().count() + 3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{AuditLog, Frame};
    use crate::runtime::Kernel;
    use crate::scope::Scope;
    use std::sync::Arc;
    use uuid::Uuid;

    async fn ensure_kernel_with_audit() -> Arc<Kernel> {
        if let Some(k) = Kernel::get() {
            if k.audit().is_some() {
                return k;
            }
        }

        let root =
            std::env::temp_dir().join(format!("abbot-head-bundle-{}", Uuid::new_v4().to_string()));
        std::fs::create_dir_all(&root).unwrap();

        let k = Kernel::get().unwrap_or_else(|| Kernel::init(&root));
        if k.audit().is_none() {
            let logs_db = root.join("logs.db");
            let audit = AuditLog::open(&logs_db).unwrap();
            k.set_audit(audit).await;
        }
        k
    }

    async fn dispatch(req: Frame) {
        let k = ensure_kernel_with_audit().await;
        let dispatcher = k.dispatcher().await;
        let mut rx = dispatcher.dispatch(
            req,
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;
    }

    #[tokio::test]
    async fn builds_conversation_with_roles() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let _ = ensure_kernel_with_audit().await;

        // Human says something
        dispatch(
            Frame::req(
                "log:append",
                serde_json::json!({
                    "kind": "chat:user",
                    "scope": "#general",
                    "data": {"content": "hello monk"}
                }),
            )
            .with_actor("human/alice"),
        )
        .await;

        // Head (Monk) responds
        dispatch(
            Frame::req(
                "log:append",
                serde_json::json!({
                    "kind": "chat:head",
                    "scope": "#general",
                    "data": {"sender": "Monk", "content": "hello alice"}
                }),
            )
            .with_actor("head/Monk"),
        )
        .await;

        // Human asks question
        dispatch(
            Frame::req(
                "log:append",
                serde_json::json!({
                    "kind": "chat:user",
                    "scope": "#general",
                    "data": {"content": "can you help?"}
                }),
            )
            .with_actor("human/alice"),
        )
        .await;

        let builder = HeadBundleBuilder::new(store, std::env::current_dir().unwrap());
        let cfg = HeadBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        assert!(matches!(messages[0].role, Role::System));
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("Head")
        );

        let conversation: Vec<&ChatMessage> = messages
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .collect();

        assert!(
            conversation.iter().any(|m| {
                matches!(m.role, Role::User)
                    && m.content.as_deref().unwrap_or("").contains("hello monk")
                    && m.content.as_deref().unwrap_or("").contains("alice")
            }),
            "expected hello monk message"
        );

        assert!(
            conversation.iter().any(|m| {
                matches!(m.role, Role::Assistant)
                    && m.content.as_deref().unwrap_or("").contains("hello alice")
            }),
            "expected assistant reply"
        );

        assert!(
            conversation.iter().any(|m| {
                matches!(m.role, Role::User)
                    && m.content.as_deref().unwrap_or("").contains("can you help")
            }),
            "expected follow-up question"
        );
    }

    #[tokio::test]
    async fn includes_task_messages() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let _ = ensure_kernel_with_audit().await;

        dispatch(
            Frame::req(
                "task:enqueue",
                serde_json::json!({
                    "task_id": "t-1",
                    "head_id": "Monk",
                    "goal": "do the thing",
                    "input": "",
                    "scope": "#general",
                    "notify_scope": "#general"
                }),
            )
            .with_actor("head/Monk"),
        )
        .await;

        dispatch(
            Frame::req(
                "task:complete",
                serde_json::json!({
                    "task_id": "t-1",
                    "ok": true,
                    "summary": "done"
                }),
            )
            .with_actor("hand/hand-1"),
        )
        .await;

        let builder = HeadBundleBuilder::new(store, std::env::current_dir().unwrap());
        let cfg = HeadBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        let has_task = messages
            .iter()
            .skip_while(|m| matches!(m.role, Role::System))
            .any(|m| {
                matches!(m.role, Role::User)
                    && m.content.as_deref().unwrap_or("").contains("task t-1")
            });
        assert!(has_task, "expected task t-1 summary to appear");
    }

    #[test]
    fn injects_user_prompt_when_cached() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        store
            .put_cached_user_prompt("abc123", "# User Prompt\nStay concise.")
            .unwrap();
        store.set_scope_user_prompt("#general", "abc123").unwrap();

        let builder = HeadBundleBuilder::new(store, std::env::current_dir().unwrap());
        let cfg = HeadBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg);

        assert!(messages.len() >= 2);
        assert!(matches!(messages[1].role, Role::System));
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or("")
                .contains("User Prompt")
        );
    }
}
