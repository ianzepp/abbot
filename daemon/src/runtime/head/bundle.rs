use std::sync::Arc;

use crate::history::Store;
use crate::kernel::{ConversationItem, FrameSelectArgs};
use crate::hal::llm::{ChatMessage, Role};
use crate::runtime::Kernel;
use crate::runtime::RuntimeSnapshot;
use crate::runtime::SnapshotManager;
use crate::runtime::{atomic_write_file_0600, read_optional_file, workspace_mind_memory};
use crate::scope::Scope;
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::runtime::SystemSlot;
use crate::runtime::{SystemBundle, SystemBundler, TarsDials};
use crate::runtime::trait_catalog;

pub struct HeadBundleConfig {
    pub head_id: String,
    pub scopes: Vec<Scope>,
    pub max_messages_per_scope: usize,
    pub context_budget_tokens: Option<u32>,
    pub traits: Vec<String>,
    pub tars: TarsDials,
    pub time_gap_marker_minutes: Option<u64>,
}

impl HeadBundleConfig {
    pub fn new(head_id: impl Into<String>, scopes: Vec<Scope>) -> Self {
        Self {
            head_id: head_id.into(),
            scopes,
            max_messages_per_scope: 100,
            context_budget_tokens: None,
            traits: Vec::new(),
            tars: TarsDials::default(),
            time_gap_marker_minutes: Some(60),
        }
    }

    pub fn with_context_budget_tokens(mut self, budget: Option<u32>) -> Self {
        self.context_budget_tokens = budget;
        self
    }

    pub fn with_traits(mut self, traits: Vec<String>) -> Self {
        self.traits = traits;
        self
    }

    pub fn with_tars(mut self, tars: TarsDials) -> Self {
        self.tars = tars;
        self
    }

    pub fn with_time_gap_marker_minutes(mut self, minutes: Option<u64>) -> Self {
        self.time_gap_marker_minutes = minutes;
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
}

impl HeadBundleBuilder {
    pub async fn new(store: Arc<Store>, workspace_root: PathBuf) -> Self {
        let snapshot = SnapshotManager::new(workspace_root.clone(), Some(store.clone())).await;
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
        Self {
            store,
            workspace_root,
            identity: identity.to_string(),
            context: context.to_string(),
            behavior: behavior.to_string(),
            snapshot,
        }
    }

    pub async fn build(&self, cfg: &HeadBundleConfig) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        let snap = self.snapshot.get();

        let ltm = self.load_global_ltm().await;

        let mut sys = SystemBundle::default();
        sys.set_slot(SystemSlot::Core, self.get_layer_0_identity());
        sys.set_slot(
            SystemSlot::Commandments,
            self.get_layer_1_commandments(&snap),
        );
        sys.set_slot(SystemSlot::Context, self.get_layer_2_context());
        sys.set_slot(SystemSlot::ToolsPrimary, self.get_layer_3_head_tools(&snap));
        sys.set_slot(
            SystemSlot::ToolsSecondary,
            self.get_layer_4_hand_tools(&snap),
        );
        sys.set_slot(
            SystemSlot::ToolsExternal,
            self.get_layer_5_external_tools(&cfg.scopes).await,
        );
        sys.set_slot(SystemSlot::Behavior, self.get_layer_6_behavior());
        sys.set_slot(
            SystemSlot::Environment,
            self.get_layer_7_environment(&snap, &cfg.scopes).await,
        );
        sys.set_slot(SystemSlot::Memory, self.get_layer_8_long_term_memory(&ltm));
        {
            let mut tone_parts = Vec::new();
            let t = cfg.tars.render();
            if !t.trim().is_empty() {
                tone_parts.push(t);
            }
            let tr = trait_catalog::render_traits(&cfg.traits);
            if !tr.trim().is_empty() {
                tone_parts.push(tr);
            }
            if !tone_parts.is_empty() {
                sys.set_slot(SystemSlot::Tone, tone_parts.join("\n\n"));
            }
        }

        let system_content = sys.render();
        let mut system_tokens = estimate_tokens(&system_content);
        messages.push(ChatMessage::new(Role::System, system_content));

        if let Some(user_prompt) = self.load_user_prompt(&cfg.scopes).await {
            let prompt_tokens = estimate_tokens(&user_prompt);
            system_tokens += prompt_tokens;
            messages.push(ChatMessage::new(Role::System, user_prompt));
        }

        // Gather and sort all messages from all scopes by timestamp.
        // Apply per-scope reset checkpoints so a client can start a fresh conversation
        // without needing to delete old logs.
        let mut all_messages: Vec<ConversationItem> = self.fetch_conversation_items(cfg).await;

        // Sort by timestamp (oldest first for conversation order)
        all_messages.sort_by_key(|m| (m.ts_ms, m.seq));

        let mut last_reset: std::collections::HashMap<String, (i64, u64)> =
            std::collections::HashMap::new();
        for m in &all_messages {
            if m.kind == "reset" {
                if let Some(ref scope) = m.scope {
                    last_reset
                        .entry(scope.clone())
                        .and_modify(|cur| {
                            if (m.ts_ms, m.seq) > *cur {
                                *cur = (m.ts_ms, m.seq)
                            }
                        })
                        .or_insert((m.ts_ms, m.seq));
                }
            }
        }

        if !last_reset.is_empty() {
            all_messages.retain(|m| {
                let Some(ref scope) = m.scope else {
                    return true;
                };
                let Some(&(ts, seq)) = last_reset.get(scope) else {
                    return true;
                };
                // Keep the reset marker itself and anything after it.
                m.kind == "reset" || (m.ts_ms, m.seq) > (ts, seq)
            });
        }

        // Convert to chat messages with appropriate roles.
        // Optionally insert sparse time-gap markers (system messages) before long-delayed
        // human messages so the head can detect conversation breaks.
        let gap_threshold_ms = cfg
            .time_gap_marker_minutes
            .and_then(|m| (m > 0).then_some(m as i64 * 60_000));
        let mut last_human_ts_ms: Option<i64> = None;

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

            if is_human {
                if let (Some(prev), Some(threshold)) = (last_human_ts_ms, gap_threshold_ms) {
                    let delta = msg.ts_ms.saturating_sub(prev);
                    if delta >= threshold {
                        let marker = format!(
                            "Time gap: {} since last user message (prev: {}, current: {})",
                            format_delta_ms(delta),
                            format_ts_utc(prev),
                            format_ts_utc(msg.ts_ms)
                        );
                        history.push((Role::System, marker, false));
                    }
                }
                last_human_ts_ms = Some(msg.ts_ms);
            }

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

    async fn fetch_conversation_items(&self, cfg: &HeadBundleConfig) -> Vec<ConversationItem> {
        let Some(k) = Kernel::get() else {
            return Vec::new();
        };
        let Some(store) = k.frames() else {
            return Vec::new();
        };

        let mut all_items: Vec<ConversationItem> = Vec::new();
        for scope in &cfg.scopes {
            let mut args = FrameSelectArgs::default();
            args.scope = Some(scope.to_string());
            args.limit = Some(cfg.max_messages_per_scope as u64);
            args.order = Some("asc".to_string());

            if let Ok((items, _)) =
                crate::kernel::frame_select::select_conversation(store.pool(), &args).await
            {
                all_items.extend(items);
            }
        }

        all_items
    }

    async fn load_global_ltm(&self) -> String {
        let path = workspace_mind_memory(&self.workspace_root);

        if let Ok(Some(content)) = read_optional_file(&path) {
            return content;
        }

        // One-time migration from legacy DB location.
        let legacy = self.store.get_head_ltm("conclave").await.unwrap_or_default();
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
        SystemBundler::render_tools_section("Head Tools", snap.head_tools_md.trim())
    }

    /// Layer 4: Explains hand tooling and how to delegate via tasks.
    ///
    /// WHY: Reinforces the delegation contract so heads offload work instead of
    /// burning context on file IO or exploration.
    fn get_layer_4_hand_tools(&self, snap: &RuntimeSnapshot) -> String {
        SystemBundler::render_tools_section(
            "Hand Tools (via head__task_create)",
            snap.hand_tools_md.trim(),
        )
    }

    /// Layer 5: Summarizes user-provided external tools (if any).
    ///
    /// WHY: Keeps untrusted client capabilities isolated in one section so the
    /// head can consciously opt into them.
    async fn get_layer_5_external_tools(&self, scopes: &[Scope]) -> String {
        let mut by_name: BTreeMap<String, String> = BTreeMap::new();
        for scope in scopes {
            let scope_str = scope.to_string();
            if let Ok(rows) = self.store.list_tool_summaries(&scope_str, "external").await {
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
        SystemBundler::render_external_tools_section(lines.trim_end())
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
    async fn get_layer_7_environment(&self, snap: &RuntimeSnapshot, scopes: &[Scope]) -> String {
        let mut out = snap.environment_md.trim().to_string();
        let mut env_blocks = Vec::new();
        for scope in scopes {
            let scope_str = scope.to_string();
            if let Ok(Some(env)) = self.store.get_session_env(&scope_str).await {
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

    // Layer 9 is rendered by the shared trait prompt renderer.

    async fn load_user_prompt(&self, scopes: &[Scope]) -> Option<String> {
        for scope in scopes {
            if let Ok(Some(prompt)) = self.store.get_scope_user_prompt(scope.as_str()).await {
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

fn format_ts_utc(ts_ms: i64) -> String {
    let Some(dt) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ts_ms) else {
        return "(unknown)".to_string();
    };
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn format_delta_ms(delta_ms: i64) -> String {
    let mut secs = (delta_ms.max(0) / 1000) as u64;
    let days = secs / 86_400;
    secs %= 86_400;
    let hours = secs / 3_600;
    secs %= 3_600;
    let mins = secs / 60;
    secs %= 60;

    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else {
        format!("{mins}m{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::Scope;
    use std::sync::Arc;

    #[tokio::test]
    async fn injects_user_prompt_when_cached() {
        let store = Arc::new(Store::open(":memory:").await.unwrap());
        store
            .put_cached_user_prompt("abc123", "# User Prompt\nStay concise.")
            .await
            .unwrap();
        store.set_scope_user_prompt("#general", "abc123").await.unwrap();

        let builder = HeadBundleBuilder::new(store, std::env::current_dir().unwrap()).await;
        let cfg = HeadBundleConfig::new("Monk", vec![Scope::from("#general")]);
        let messages = builder.build(&cfg).await;

        assert!(messages.len() >= 2);
        assert!(matches!(messages[1].role, Role::System));
        assert!(messages[1]
            .content
            .as_deref()
            .unwrap_or("")
            .contains("User Prompt"));
    }
}
