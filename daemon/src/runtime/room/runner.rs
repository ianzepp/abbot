//! Room Runner - Core multi-round deliberation engine
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The runner executes a bounded deliberation loop where participants take turns
//! responding to context, making proposals, casting votes, and signaling consensus.
//! It replaces the Conclave engine with a generic round-based loop that uses
//! tool-like operations (propose, vote, done) parsed from JSON responses.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Round-bounded iteration: Each room type has a max_rounds limit (conclave=5,
//!   autonomy=3, work=10). This prevents runaway LLM costs while giving rooms
//!   enough rounds to reach consensus.
//! - Participant-sequential, round-parallel: Within each round, participants
//!   speak sequentially (each sees prior responses). Across rounds, the loop
//!   checks for consensus after all participants have spoken.
//! - Worktree isolation: Work rooms get a dedicated git worktree provisioned
//!   before deliberation starts and cleaned up after completion.
//!
//! TRADE-OFFS
//! ==========
//! - Sequential participant queries (not parallel): Each participant sees the
//!   full transcript including earlier participants in the same round. This
//!   enables richer deliberation but increases latency linearly with participant
//!   count.
//! - JSON response parsing: Participants return structured JSON rather than using
//!   real tool calls. This simplifies the LLM interaction (single chat turn per
//!   participant) but requires robust fallback parsing for malformed responses.
//! - Decision execution is fire-and-forget for needs: Needs are dispatched via
//!   syscalls but the runner doesn't wait for them to complete. This prevents
//!   room execution from blocking on potentially long-running need fulfillment.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::history::Store;
use crate::kernel::Frame;
use crate::hal::llm::{ChatMessage, Role};
use crate::runtime::Kernel;
use crate::runtime::{
    atomic_write_file_0600, bump_reboot_epoch, read_optional_file, workspace_mind_memory,
    workspace_mind_self,
};
use crate::scope::Scope;

use super::bundle::{RoomBundleBuilder, RoomBundleConfig, RoomType, WakeMode};
use super::config::RoomConfig;
use super::tools::{self, ProposalTracker, coordination_tool_specs, work_tool_specs};
use super::types::{
    ControlProposal, LtmProposal, Participant, Room, RoomDecision, SelfProposal,
};
use super::worktree::WorktreeManager;

// =============================================================================
// GRAMMAR PROMPTS
// =============================================================================
//
// WHY compile-time inclusion: Embeds the deliberation grammar (response format
// instructions) into the binary, eliminating runtime file-not-found errors.
// Each room type has a distinct grammar reflecting its available operations.

const ROOM_CONCLAVE_GRAMMAR: &str = include_str!("room_conclave.md");
const ROOM_AUTONOMY_GRAMMAR: &str = include_str!("room_autonomy.md");
const ROOM_WORK_GRAMMAR: &str = include_str!("room_work.md");

// =============================================================================
// RUNNER
// =============================================================================
//
// WHY a dedicated runner struct: Encapsulates all dependencies (store, scopes,
// workspace, config) needed for deliberation so that callers (coordinator,
// syscalls) only need to construct and call run().

/// Executes multi-round deliberation for a room until consensus or timeout.
///
/// WHY this exists: Centralizes the deliberation loop so that all room types
/// (conclave, autonomy, work) share the same execution engine, differing only
/// in configuration (max_rounds, grammar, tool specs).
pub struct RoomRunner {
    store: Arc<Store>,
    scopes: Vec<Scope>,
    workspace: PathBuf,
    config: RoomConfig,
}

impl RoomRunner {
    pub fn new(
        store: Arc<Store>,
        scopes: Vec<Scope>,
        workspace: PathBuf,
        config: RoomConfig,
    ) -> Self {
        Self {
            store,
            scopes,
            workspace,
            config,
        }
    }

    // =========================================================================
    // CORE DELIBERATION LOOP
    // =========================================================================
    //
    // WHY this structure: The loop iterates rounds, within each round iterates
    // participants, collects proposals/votes, and checks for consensus. This
    // mirrors a real deliberation where each speaker sees what came before.

    /// Execute the deliberation loop for a room.
    ///
    /// WHY Option return: Returns None if the room produced no actionable
    /// decision (empty timeout with no approved proposals). Callers can
    /// distinguish "nothing to do" from "decision made" without inspecting
    /// the decision contents.
    pub async fn run(
        &self,
        room: &mut Room,
        _trace: Option<()>,
    ) -> Option<RoomDecision> {
        // ---------------------------------------------------------------------
        // PHASE 1: WORKTREE PROVISIONING
        // WHY before deliberation: Work rooms need filesystem isolation before
        // any tools can execute. Provisioning here ensures the worktree exists
        // for the entire lifetime of the room.
        // ---------------------------------------------------------------------
        let worktree_path = if room.room_type == super::types::RoomType::Work {
            let mgr = WorktreeManager::new(&self.workspace);
            match mgr.provision(&room.id, None) {
                Ok(path) => {
                    tracing::info!(room_id = %room.id, path = %path.display(), "provisioned worktree");
                    Some(path)
                }
                Err(e) => {
                    tracing::error!(room_id = %room.id, error = %e, "failed to provision worktree");
                    return None;
                }
            }
        } else {
            None
        };

        // ---------------------------------------------------------------------
        // PHASE 2: CONTEXT ASSEMBLY
        // WHY separate phase: Context building involves store queries, file
        // reads, and bundle construction. Doing it once before the loop avoids
        // redundant work across rounds.
        // ---------------------------------------------------------------------
        let context = self.build_context(
            match room.room_type {
                super::types::RoomType::Conclave => WakeMode::Normal,
                super::types::RoomType::Autonomy => WakeMode::Normal,
                super::types::RoomType::Work => WakeMode::Normal,
            },
            match room.room_type {
                super::types::RoomType::Conclave => RoomType::Conclave,
                super::types::RoomType::Autonomy => RoomType::Autonomy,
                super::types::RoomType::Work => RoomType::Work,
            },
        );

        let grammar = match room.room_type {
            super::types::RoomType::Conclave => ROOM_CONCLAVE_GRAMMAR,
            super::types::RoomType::Autonomy => ROOM_AUTONOMY_GRAMMAR,
            super::types::RoomType::Work => ROOM_WORK_GRAMMAR,
        };

        let _tool_specs = match room.room_type {
            super::types::RoomType::Work => work_tool_specs(),
            _ => coordination_tool_specs(),
        };

        let participants = room.participants.clone();
        let participant_names: Vec<String> = participants.iter().map(|p| p.name.clone()).collect();

        let mut tracker = ProposalTracker::default();

        // ---------------------------------------------------------------------
        // PHASE 3: MULTI-ROUND DELIBERATION
        // WHY round-based: Each round gives all participants a chance to speak,
        // propose, and vote. Multiple rounds allow iterative refinement until
        // consensus emerges or the budget is exhausted.
        // ---------------------------------------------------------------------
        for round in 0..room.max_rounds {
            tracing::debug!(room_id = %room.id, round = round, "room round");

            let mut round_all_done = true;

            for participant in &participants {
                // WHY skip done participants: Once a participant signals consensus,
                // their position is fixed. Re-querying wastes LLM calls.
                if tracker.done_signals.contains(&participant.name) {
                    continue;
                }

                let transcript_so_far = self.format_transcript(&room.transcript);
                let proposals_summary = self.format_proposals(&tracker);

                let response = match self
                    .query_participant(
                        participant,
                        &context,
                        &transcript_so_far,
                        &proposals_summary,
                        grammar,
                        &room.id,
                        round,
                    )
                    .await
                {
                    Some(r) => r,
                    None => {
                        round_all_done = false;
                        continue;
                    }
                };

                let parsed = parse_mind_response(&response);

                tracing::info!(
                    participant = %participant.name,
                    thoughts = %truncate(&parsed.thoughts, 200),
                    proposals = parsed.proposals.len(),
                    consensus = parsed.consensus,
                    "participant speaks"
                );

                room.add_message(&participant.name, &response, round);

                // Process proposals through tracker
                for p in &parsed.proposals {
                    let args = json!({
                        "proposal_type": p.kind,
                        "text": p.text,
                        "context": p.context,
                        "priority": p.priority,
                        "content": p.content,
                        "pattern": p.pattern,
                        "mode": p.mode,
                    });
                    tracker.handle_propose(&args, &participant.name);
                }

                // Process votes
                for (key, vote) in &parsed.votes {
                    let args = json!({
                        "proposal_key": key,
                        "vote": vote,
                    });
                    tracker.handle_vote(&args, &participant.name);
                }

                if parsed.consensus {
                    tracker.done_signals.insert(participant.name.clone());
                }

                if !parsed.consensus {
                    round_all_done = false;
                }
            }

            // WHY check both flags: round_all_done catches the case where all
            // participants signaled consensus in this round. tracker.all_done()
            // catches accumulated done signals across rounds.
            if round_all_done || tracker.all_done(&participant_names) {
                tracing::debug!(room_id = %room.id, round = round, "room reached consensus");
                let decision = tracker.tally_decision();
                room.close(decision.clone());
                self.save_room(&room.id, "consensus", &room.transcript, &decision);
                self.execute_decision(&decision).await;
                self.cleanup_worktree(&room.id, &worktree_path);
                return Some(decision);
            }
        }

        // ---------------------------------------------------------------------
        // PHASE 4: TIMEOUT FALLBACK
        // WHY partial execution: Even on timeout, approved proposals (those with
        // enough votes) are still executed. This prevents wasted deliberation
        // when consensus was close but not unanimous.
        // ---------------------------------------------------------------------
        tracing::warn!(room_id = %room.id, "room timed out");
        room.timeout();

        let decision = tracker.tally_decision();
        self.save_room(&room.id, "timeout", &room.transcript, &decision);
        self.cleanup_worktree(&room.id, &worktree_path);
        if !decision.needs.is_empty() || !decision.wants.is_empty() {
            self.execute_decision(&decision).await;
            return Some(decision);
        }

        None
    }

    /// Clean up worktree if one was provisioned.
    ///
    /// WHY: Worktree cleanup must happen on both consensus and timeout paths.
    /// Extracting it avoids duplicating cleanup logic at each exit point.
    fn cleanup_worktree(&self, room_id: &str, worktree_path: &Option<PathBuf>) {
        if worktree_path.is_some() {
            let mgr = WorktreeManager::new(&self.workspace);
            if let Err(e) = mgr.cleanup(room_id) {
                tracing::warn!(room_id = %room_id, error = %e, "failed to cleanup worktree");
            }
        }
    }

    // =========================================================================
    // CONTEXT BUILDING
    // =========================================================================
    //
    // WHY separate from run(): Context assembly involves store queries, file
    // reads, and bundle construction — complex enough to warrant its own section.
    // The bundle builder handles all the layered context (system prompt, scopes,
    // history, LTM, self-identity, activity, GitHub state).

    /// Build the combined system + user context for a room session.
    ///
    /// WHY bundle-based: The RoomBundleBuilder encapsulates all context-gathering
    /// logic (scopes, history, LTM, activity, GitHub). Using it here ensures
    /// rooms get the same rich context as the old Conclave engine.
    fn build_context(&self, wake_mode: WakeMode, room_type: RoomType) -> String {
        let bundle_builder = RoomBundleBuilder::new(self.store.clone());
        let bundle_cfg = RoomBundleConfig::new("conclave", self.scopes.clone())
            .with_wake_mode(wake_mode)
            .with_workspace(self.workspace.clone())
            .with_fever(self.config.fever.clone())
            .with_filter(self.config.filter.clone())
            .with_poverty(self.config.poverty.clone())
            .with_room_type(room_type);
        let messages = bundle_builder.build(&bundle_cfg);

        let mut parts = Vec::new();
        if let Some(system) = messages.iter().find(|m| matches!(m.role, Role::System)) {
            if let Some(content) = &system.content {
                parts.push(content.clone());
            }
        }
        if let Some(user) = messages.iter().find(|m| matches!(m.role, Role::User)) {
            if let Some(content) = &user.content {
                parts.push(content.clone());
            }
        }
        parts.join("\n\n")
    }

    // =========================================================================
    // TRANSCRIPT FORMATTING
    // =========================================================================
    //
    // WHY formatting helpers: Participants receive the transcript and proposals
    // as plain text in their user prompt. These helpers convert internal structs
    // into human-readable summaries that LLMs can reason about effectively.

    /// Format the room transcript as a readable discussion log.
    fn format_transcript(&self, transcript: &[super::types::RoomMessage]) -> String {
        if transcript.is_empty() {
            return "(no discussion yet)".to_string();
        }
        transcript
            .iter()
            .map(|m| format!("[{}] {}", m.mind, m.content))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Format current proposals with vote tallies for participant context.
    fn format_proposals(&self, tracker: &ProposalTracker) -> String {
        if tracker.proposals.is_empty() {
            return "(no proposals yet)".to_string();
        }
        tracker
            .proposals
            .iter()
            .map(|(proposer, p)| {
                let key = format!("{}:{}", p.proposal_type, p.text);
                let vote_summary = tracker
                    .votes
                    .get(&key)
                    .map(|v| {
                        v.iter()
                            .map(|(m, vote)| format!("{}={}", m, vote))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_else(|| "no votes".to_string());
                format!(
                    "- [{}] {} (priority: {}, by: {}, votes: {})",
                    p.proposal_type, p.text, p.priority, proposer, vote_summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // =========================================================================
    // LLM QUERY
    // =========================================================================
    //
    // WHY dispatch via kernel: Rather than calling the LLM provider directly,
    // queries go through the kernel's dispatcher as `llm:chat` syscalls. This
    // ensures rate limiting, logging, and provider abstraction are applied
    // consistently with all other LLM calls in the system.

    /// Query a single participant for their response to the current room state.
    ///
    /// WHY Option return: Returns None on LLM failure (not configured, error,
    /// empty response) rather than panicking. The caller marks the round as
    /// incomplete, giving the participant another chance in the next round.
    async fn query_participant(
        &self,
        participant: &Participant,
        context: &str,
        transcript: &str,
        proposals: &str,
        grammar: &str,
        room_id: &str,
        round: usize,
    ) -> Option<String> {
        let room_cfg = RoomConfig::from_config();

        if !room_cfg.llm.enabled {
            tracing::warn!(participant = %participant.name, "room LLM not configured, skipping query");
            return None;
        }

        let system = format!("{}\n\n{}", participant.system_prompt, grammar);

        let user_prompt = format!(
            "## Current Context\n\n{}\n\n## Discussion So Far\n\n{}\n\n## Proposals On The Table\n\n{}\n\nRespond with your thoughts, any new proposals, your votes, and whether you believe we have consensus.",
            context, transcript, proposals
        );

        let messages = vec![
            ChatMessage::new(Role::System, system),
            ChatMessage::new(Role::User, user_prompt),
        ];

        let Some(k) = Kernel::get() else {
            tracing::error!(participant = %participant.name, "kernel not initialized");
            return None;
        };

        let dispatcher = k.dispatcher().await;
        let req = Frame::req("llm:chat", json!({"messages": messages}))
            .with_actor(format!("mind/{}", participant.name));

        // WHY streaming accumulation: The llm:chat syscall returns text_delta
        // events. Accumulating them here keeps the runner decoupled from the
        // specific streaming protocol used by the LLM provider.
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let mut content = String::new();
        while let Some(frame) = rx.recv().await {
            match frame.op {
                crate::kernel::FrameOp::Item => {
                    let Some(data) = frame.data.as_ref() else { continue };
                    if data.get("type").and_then(|v| v.as_str()) == Some("text_delta") {
                        if let Some(text) = data.get("content").and_then(|v| v.as_str()) {
                            content.push_str(text);
                        }
                    }
                }
                crate::kernel::FrameOp::Done => break,
                crate::kernel::FrameOp::Error => {
                    let msg = frame
                        .data
                        .as_ref()
                        .and_then(|d| d.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("llm error");
                    tracing::error!(participant = %participant.name, error = %msg, "room query failed");
                    return None;
                }
                _ => {}
            }
        }

        if content.trim().is_empty() {
            tracing::error!(participant = %participant.name, "room query returned no content");
            return None;
        }

        Some(content)
    }

    // =========================================================================
    // PERSISTENCE
    // =========================================================================
    //
    // WHY persist rooms: Room transcripts and decisions are saved to SQLite for
    // post-hoc analysis, debugging, and continuity across restarts. Uses the
    // existing `save_conclave` store method for backward compatibility.

    /// Save room transcript and decision to the store.
    fn save_room(
        &self,
        room_id: &str,
        status: &str,
        transcript: &[super::types::RoomMessage],
        decision: &RoomDecision,
    ) {
        let transcript_json = serde_json::to_string(transcript).unwrap_or_default();
        let decision_json = serde_json::to_string(decision).unwrap_or_default();
        if let Err(e) = self.store.save_conclave(room_id, status, &transcript_json, &decision_json) {
            tracing::error!(error = %e, "failed to save room");
        }
    }

    // =========================================================================
    // DECISION EXECUTION
    // =========================================================================
    //
    // WHY post-deliberation execution: Approved proposals are side effects
    // (enqueue needs, persist wants, update LTM/self, trigger reboots). These
    // are executed after the room closes to maintain a clean separation between
    // deliberation (pure reasoning) and execution (state mutation).

    /// Execute all approved proposals from the room's decision.
    ///
    /// WHY sequential execution: Needs are dispatched via syscalls, wants are
    /// persisted to the store, and LTM/self/control ops modify files or global
    /// state. Sequential execution prevents race conditions between operations
    /// that may affect the same state (e.g., two LTM appends to the same file).
    async fn execute_decision(&self, decision: &RoomDecision) {
        // ---------------------------------------------------------------------
        // NEEDS: Dispatch via kernel syscalls
        // WHY syscall dispatch: Needs enter the kernel's need queue where they
        // are prioritized and assigned to heads. This integrates room decisions
        // with the main work scheduling pipeline.
        // ---------------------------------------------------------------------
        for need in &decision.needs {
            let need_id = uuid::Uuid::new_v4().to_string();
            let priority = match need.priority.as_str() {
                "low" | "high" | "urgent" | "normal" => need.priority.as_str(),
                _ => "normal",
            };

            if let Some(k) = Kernel::get() {
                let dispatcher = k.dispatcher().await;
                let req = Frame::req(
                    "need:enqueue",
                    json!({
                        "need_id": need_id,
                        "source": "room",
                        "priority": priority,
                        "need": need.need.clone(),
                        "context": need.context.clone(),
                        "scope": "main",
                        "reconvene": need.reconvene,
                    }),
                )
                .with_actor("system/room_runner");

                let mut rx = dispatcher.dispatch(
                    req,
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    CancellationToken::new(),
                );
                let _ = rx.recv().await;
            }

            tracing::info!(need = %need.need, priority = %priority, "room proposes need");
        }

        // ---------------------------------------------------------------------
        // WANTS: Persist to store
        // WHY store-based: Wants are aspirational items that persist across
        // sessions. Storing them in SQLite ensures they survive restarts.
        // ---------------------------------------------------------------------
        for want in &decision.wants {
            let want_id = uuid::Uuid::new_v4().to_string();
            if let Err(e) = self.store.add_want(
                &want_id,
                &want.want,
                &want.context,
                &want.priority,
                "room",
            ) {
                tracing::error!(error = %e, "failed to add want");
            }
        }

        // ---------------------------------------------------------------------
        // LTM/SELF/CONTROL: File and state mutations
        // WHY separate methods: Each operation type has distinct semantics
        // (file append/replace/remove for LTM/self, epoch bump for control).
        // Separate methods keep the logic readable and testable.
        // ---------------------------------------------------------------------
        self.apply_ltm_ops(&decision.ltm_ops);
        self.apply_self_ops(&decision.self_ops);
        self.apply_control_ops(&decision.control_ops).await;
    }

    // =========================================================================
    // CONTROL OPERATIONS
    // =========================================================================
    //
    // WHY separate from LTM/self: Control operations (currently only reboot)
    // affect global state (the reboot epoch) rather than per-workspace files.
    // They also require async for potential future expansions.

    /// Apply approved control proposals (e.g., reboot_collective).
    ///
    /// WHY break after first: Only one reboot per decision makes sense.
    /// Multiple reboot proposals would be redundant.
    async fn apply_control_ops(&self, ops: &[ControlProposal]) {
        for op in ops {
            if op.kind != "reboot_collective" {
                continue;
            }
            let reason = if op.reason.trim().is_empty() {
                "requested by room"
            } else {
                op.reason.trim()
            };
            let epoch = bump_reboot_epoch();
            tracing::warn!(epoch, mode = %op.mode, proposer = %op.proposer, reason = %reason, "reboot requested");
            break;
        }
    }

    // =========================================================================
    // LTM OPERATIONS
    // =========================================================================
    //
    // WHY file-based LTM: Long-term memory is stored in `.abbot/mind/memory.md`
    // as plain text. This makes it human-readable, git-trackable, and editable
    // outside of the system. The legacy fallback reads from SQLite for migration.

    /// Apply approved LTM proposals (append, replace, remove) to memory.md.
    ///
    /// WHY legacy fallback: If the file doesn't exist yet, checks the SQLite
    /// store for pre-migration LTM content and seeds the file. This enables
    /// seamless migration from the old storage format.
    fn apply_ltm_ops(&self, ops: &[LtmProposal]) {
        if ops.is_empty() {
            return;
        }

        let path = workspace_mind_memory(&self.workspace);
        if !path.parent().map(|p| p.exists()).unwrap_or(false) {
            tracing::warn!("cannot resolve mind/memory.md for workspace");
            return;
        }

        let current = match read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // WHY legacy migration: Seeds the file from SQLite on first access
                let legacy = self.store.get_head_ltm("conclave").unwrap_or_default();
                if !legacy.trim().is_empty() {
                    let _ = atomic_write_file_0600(&path, legacy.trim());
                    legacy
                } else {
                    String::new()
                }
            }
            Err(_) => String::new(),
        };

        let mut ltm = current.clone();

        for op in ops {
            match op.kind.as_str() {
                "append" => {
                    let content = op.content.trim();
                    if content.is_empty() { continue; }
                    if !ltm.is_empty() { ltm.push_str("\n\n"); }
                    ltm.push_str(content);
                }
                "replace" => {
                    if op.pattern.is_empty() { continue; }
                    if let Some(pos) = ltm.find(&op.pattern) {
                        let end = pos + op.pattern.len();
                        ltm.replace_range(pos..end, &op.content);
                    }
                }
                "remove" => {
                    if op.pattern.is_empty() { continue; }
                    if ltm.contains(&op.pattern) {
                        ltm = ltm.replace(&op.pattern, "");
                        // WHY collapse whitespace: Removing a section can leave
                        // triple-newlines. Collapsing them keeps the file clean.
                        while ltm.contains("\n\n\n") {
                            ltm = ltm.replace("\n\n\n", "\n\n");
                        }
                        ltm = ltm.trim().to_string();
                    }
                }
                _ => {}
            }
        }

        if ltm != current {
            if let Err(e) = atomic_write_file_0600(&path, &ltm) {
                tracing::error!(error = %e, "failed to save mind/memory.md");
            }
        }
    }

    // =========================================================================
    // SELF-IDENTITY OPERATIONS
    // =========================================================================
    //
    // WHY separate from LTM: Self-identity (`.abbot/mind/self.md`) represents
    // the collective's self-concept, distinct from factual long-term memory.
    // Same file operations (append/replace/remove) but different file and
    // different legacy migration source.

    /// Apply approved self-identity proposals to self.md.
    fn apply_self_ops(&self, ops: &[SelfProposal]) {
        if ops.is_empty() {
            return;
        }

        let path = workspace_mind_self(&self.workspace);
        if !path.parent().map(|p| p.exists()).unwrap_or(false) {
            tracing::warn!("cannot resolve mind/self.md for workspace");
            return;
        }

        let current = match read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // WHY legacy migration: Seeds from SQLite conclave_self on first access
                let legacy = self.store.get_conclave_self().unwrap_or_default();
                if !legacy.trim().is_empty() {
                    let _ = atomic_write_file_0600(&path, legacy.trim());
                    legacy
                } else {
                    String::new()
                }
            }
            Err(_) => String::new(),
        };

        let mut identity = current.clone();

        for op in ops {
            match op.kind.as_str() {
                "append" => {
                    let content = op.content.trim();
                    if content.is_empty() { continue; }
                    if !identity.is_empty() { identity.push_str("\n\n"); }
                    identity.push_str(content);
                }
                "replace" => {
                    if op.pattern.is_empty() { continue; }
                    if let Some(pos) = identity.find(&op.pattern) {
                        let end = pos + op.pattern.len();
                        identity.replace_range(pos..end, &op.content);
                    }
                }
                "remove" => {
                    if op.pattern.is_empty() { continue; }
                    if identity.contains(&op.pattern) {
                        identity = identity.replace(&op.pattern, "");
                        while identity.contains("\n\n\n") {
                            identity = identity.replace("\n\n\n", "\n\n");
                        }
                        identity = identity.trim().to_string();
                    }
                }
                _ => {}
            }
        }

        if identity != current {
            if let Err(e) = atomic_write_file_0600(&path, &identity) {
                tracing::error!(error = %e, "failed to save mind/self.md");
            }
        }
    }
}

// =============================================================================
// RESPONSE PARSING
// =============================================================================
//
// WHY custom parsing: Participants return JSON-structured responses embedded in
// their natural language output. This parser handles multiple formats: fenced
// ```json blocks, raw JSON objects, and plain text fallback. This resilience is
// critical because LLM output format is inherently unreliable.

#[derive(Debug, Default)]
struct ParsedResponse {
    thoughts: String,
    proposals: Vec<ParsedProposal>,
    votes: std::collections::HashMap<String, String>,
    consensus: bool,
}

#[derive(Debug)]
struct ParsedProposal {
    kind: String,
    text: String,
    context: String,
    priority: String,
    content: String,
    pattern: String,
    mode: String,
}

/// Parse a participant's raw LLM response into structured proposals/votes.
///
/// WHY graceful degradation: If JSON parsing fails entirely, the raw text
/// becomes the "thoughts" field with no proposals or votes. This ensures
/// the deliberation continues even when a participant produces malformed output.
fn parse_mind_response(raw: &str) -> ParsedResponse {
    let json_str = extract_json(raw);
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_str) else {
        return ParsedResponse {
            thoughts: raw.to_string(),
            ..Default::default()
        };
    };

    let thoughts = v.get("thoughts").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let consensus = v.get("consensus").and_then(|v| v.as_bool()).unwrap_or(false);

    let mut proposals = Vec::new();
    if let Some(arr) = v.get("proposals").and_then(|v| v.as_array()) {
        for p in arr {
            let kind = p.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let context = p.get("context").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let priority = p.get("priority").and_then(|v| v.as_str()).unwrap_or("normal").to_string();
            let content = p.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let pattern = p.get("pattern").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mode = p.get("mode").and_then(|v| v.as_str()).unwrap_or("").to_string();
            proposals.push(ParsedProposal { kind, text, context, priority, content, pattern, mode });
        }
    }

    let mut votes = std::collections::HashMap::new();
    if let Some(obj) = v.get("votes").and_then(|v| v.as_object()) {
        for (k, val) in obj {
            if let Some(vote) = val.as_str() {
                votes.insert(k.clone(), vote.to_string());
            }
        }
    }

    ParsedResponse { thoughts, proposals, votes, consensus }
}

// =============================================================================
// HELPERS
// =============================================================================

/// Extract JSON from an LLM response that may contain surrounding text.
///
/// WHY multi-strategy: LLMs inconsistently format JSON — sometimes in fenced
/// code blocks, sometimes as bare objects, sometimes mixed with prose. Trying
/// fenced blocks first (most reliable) then falling back to brace-matching
/// maximizes successful extraction.
fn extract_json(content: &str) -> String {
    // WHY fenced first: ```json blocks are unambiguous delimiters
    if let Some(start) = content.find("```json") {
        if let Some(end) = content[start..]
            .find("```\n")
            .or_else(|| content[start..].rfind("```"))
        {
            let json_start = start + 7;
            let json_end = start + end;
            if json_end > json_start {
                return content[json_start..json_end].trim().to_string();
            }
        }
    }

    // WHY brace fallback: Some LLMs emit bare JSON without fencing
    if let Some(start) = content.find('{') {
        if let Some(end) = content.rfind('}') {
            if end > start {
                return content[start..=end].to_string();
            }
        }
    }

    content.to_string()
}

/// Truncate a string for log output, collapsing newlines.
fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        return s;
    }
    let clipped: String = s.chars().take(max).collect();
    format!("{}...", clipped)
}
