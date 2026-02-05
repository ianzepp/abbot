//! Conclave - Multi-agent deliberation system for autonomous decision-making
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The Conclave implements a multi-round deliberation protocol where Mind personas
//! (MindManager, HeadManager, HandManager) propose and vote on actions. This is
//! the core autonomy mechanism introduced in the syscall refactor to enable the
//! system to self-direct without external prompts.
//!
//! On each mind tick, the conclave convenes:
//! 1. Build context (recent activity, LTM, wants pool, GitHub issues if enabled)
//! 2. Each Mind persona receives context and responds with proposals and votes
//! 3. Iterate for up to max_rounds until consensus is reached
//! 4. Tally votes (2/3 threshold) and execute agreed needs/wants/LTM/self ops
//!
//! The Conclave supports two room types:
//! - Conclave: Strategic meeting for needs, wants, LTM updates (uses room_conclave.md)
//! - Autonomy: Operational meeting including GitHub integration (uses room_autonomy.md)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Consensus-driven: Proposals require 2/3 votes to execute (robust against single-mind errors)
//! - Multi-round deliberation: Minds see each other's proposals and votes, enabling negotiation
//! - Proposer implicit yes: The mind proposing an action implicitly votes yes for it
//! - Timeout fallback: If max_rounds is reached without consensus, execute 2/3-voted actions anyway
//! - Trace support: Optional tracing for observability of deliberation process
//!
//! TRADE-OFFS
//! ==========
//! - Multiple LLM calls vs single-mind decision: We chose multi-mind consensus for
//!   robustness and reduced hallucination risk. The cost is 3x LLM calls per round.
//! - 2/3 threshold vs unanimity: 2/3 balances consensus with progress. Unanimity
//!   would stall on disagreement; simple majority would be too aggressive.
//! - Max rounds limit: Prevents infinite loops but may cut off legitimate deliberation.
//!   Current limit (3 rounds for conclave, 2 for autonomy) balances cost and quality.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::Scope;
use crate::history::Store;
use crate::kernel::Frame;
use crate::llm::{ChatMessage, Role};
use crate::runtime::Kernel;
use crate::runtime::bump_reboot_epoch;
use crate::runtime::{
    atomic_write_file_0600, read_optional_file, workspace_mind_memory, workspace_mind_self,
};

use super::mind_bundle::{FeverMode, RoomType, WakeMode};
use super::FilterMode;
use super::room::{
    ControlProposal, LtmProposal, MindPersona, NeedProposal, Room, RoomDecision, SelfProposal,
    WantProposal,
};
use super::{MindBundleBuilder, MindBundleConfig, MindConfig};

// =============================================================================
// CONSTANTS
// =============================================================================

const ROOM_CONCLAVE_GRAMMAR: &str = include_str!("room_conclave.md");
const ROOM_AUTONOMY_GRAMMAR: &str = include_str!("room_autonomy.md");

// =============================================================================
// TRACE SUPPORT
// =============================================================================
//
// WHY: Enables observability of the deliberation process for debugging and
// monitoring. Trace events are emitted as frames for consumption by TUI/logs.

#[derive(Clone)]
pub struct ConclaveTrace {
    call_id: Uuid,
    tx: mpsc::Sender<Frame>,
    actor: String,
}

impl ConclaveTrace {
    /// Create a new trace for a conclave session.
    ///
    /// WHY: Traces are keyed by call_id and actor to correlate events with
    /// the originating syscall and runtime component.
    pub fn new(call_id: Uuid, tx: mpsc::Sender<Frame>, actor: impl Into<String>) -> Self {
        Self {
            call_id,
            tx,
            actor: actor.into(),
        }
    }

    /// Emit a trace event.
    ///
    /// WHY: Uses frame event mechanism to integrate with existing monitoring
    /// infrastructure (TUI, audit logs).
    async fn event(&self, kind: &str, data: serde_json::Value) {
        let _ = self
            .tx
            .send(
                Frame::event(
                    self.call_id,
                    json!({
                        "kind": kind,
                        "data": data,
                    }),
                )
                .with_actor(self.actor.clone())
                .with_name("mind:conclave"),
            )
            .await;
    }
}

// =============================================================================
// CONCLAVE STRUCTURE
// =============================================================================

/// The Conclave orchestrates multi-mind deliberation.
///
/// WHY: Encapsulates all state needed for a deliberation session (store for
/// history, scopes for context, workspace for file access, fever/filter/poverty
/// for LLM tuning).
pub struct Conclave {
    store: Arc<Store>,
    scopes: Vec<Scope>,
    workspace: PathBuf,
    fever: FeverMode,
    filter: FilterMode,
    poverty: super::PovertyMode,
}

// =============================================================================
// DELIBERATION TYPES
// =============================================================================
//
// WHY: These types define the protocol for multi-mind deliberation. Each Mind
// responds with thoughts, proposals, votes, and a consensus signal.

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MindResponse {
    #[serde(default)]
    thoughts: String,
    #[serde(default)]
    proposals: Vec<Proposal>,
    #[serde(default)]
    votes: HashMap<String, String>,
    #[serde(default)]
    consensus: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Proposal {
    #[serde(rename = "type")]
    kind: String,
    text: String,
    #[serde(default)]
    context: String,
    #[serde(default)]
    priority: String,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    content: String, // for ltm: what to add/replace with
    #[serde(default)]
    pattern: String, // for ltm: what to find (replace/remove)
}

// =============================================================================
// CONCLAVE IMPLEMENTATION
// =============================================================================

impl Conclave {
    /// Create a new Conclave with default config from abbot.toml.
    ///
    /// WHY: Loads fever/filter/poverty settings from config to control LLM
    /// behavior (creativity, context filtering, token budget).
    pub fn new(store: Arc<Store>, scopes: Vec<Scope>, workspace: PathBuf) -> Self {
        let mind_cfg = MindConfig::from_config();
        Self {
            store,
            scopes,
            workspace,
            fever: mind_cfg.fever,
            filter: mind_cfg.filter,
            poverty: mind_cfg.poverty,
        }
    }

    pub fn with_fever(mut self, fever: FeverMode) -> Self {
        self.fever = fever;
        self
    }

    pub fn with_filter(mut self, filter: FilterMode) -> Self {
        self.filter = filter;
        self
    }

    pub fn with_poverty(mut self, poverty: super::PovertyMode) -> Self {
        self.poverty = poverty;
        self
    }

    /// Convene a strategic meeting (conclave) to decide on needs/wants/LTM ops.
    ///
    /// WHY: This is the main entry point for autonomous strategic decision-making.
    /// Called by the mind tick system or explicitly by syscalls.
    pub async fn convene(&self, room_id: &str, wake_mode: WakeMode) -> Option<RoomDecision> {
        self.convene_with_trace(room_id, wake_mode, None).await
    }

    /// Convene with trace support for observability.
    ///
    /// WHY separate method: Tracing adds overhead; callers can opt in/out.
    pub async fn convene_with_trace(
        &self,
        room_id: &str,
        wake_mode: WakeMode,
        trace: Option<ConclaveTrace>,
    ) -> Option<RoomDecision> {
        // -------------------------------------------------------------------------
        // PHASE 1: ROOM SETUP
        // WHY: Create a Room with mind personas and max_rounds limit. The Room
        // structure encapsulates deliberation state (transcript, participants).
        // -------------------------------------------------------------------------
        let mut room = Room::conclave(room_id);

        let context = self.build_context(wake_mode, RoomType::Conclave);

        let mut all_proposals: Vec<(String, Proposal)> = Vec::new();
        let mut all_votes: HashMap<String, HashMap<String, String>> = HashMap::new();

        let minds = room.minds.clone();

        // -------------------------------------------------------------------------
        // PHASE 2: MULTI-ROUND DELIBERATION
        // WHY: Each round queries all minds, collects proposals/votes, and checks
        // for consensus. Minds see the transcript and proposals from prior rounds.
        // -------------------------------------------------------------------------
        for round in 0..room.max_rounds {
            tracing::debug!(room_id = %room_id, round = round, "conclave round");

            if let Some(t) = trace.as_ref() {
                t.event(
                    "mind:round_start",
                    json!({"room_id": room_id, "round": round, "type": "conclave"}),
                )
                .await;
            }

            let mut round_consensus = true;

            for persona in &minds {
                let transcript_so_far = self.format_transcript(&room.transcript);
                let proposals_summary = self.format_proposals(&all_proposals, &all_votes);

                if let Some(t) = trace.as_ref() {
                    t.event(
                        "mind:query",
                        json!({"room_id": room_id, "round": round, "mind": persona.name}),
                    )
                    .await;
                }

                let response = match self
                    .query_mind(
                        persona,
                        &context,
                        &transcript_so_far,
                        &proposals_summary,
                        room_id,
                        round,
                        trace.as_ref(),
                    )
                    .await
                {
                    Some(r) => r,
                    None => continue,
                };

                if let Some(t) = trace.as_ref() {
                    t.event(
                        "mind:response",
                        json!({
                            "room_id": room_id,
                            "round": round,
                            "mind": persona.name,
                            "consensus": response.consensus,
                            "proposals": response.proposals.len(),
                        }),
                    )
                    .await;
                }

                // Log persona's thoughts
                tracing::info!(
                    persona = %persona.name,
                    thoughts = %truncate(&response.thoughts, 200),
                    proposals = response.proposals.len(),
                    consensus = response.consensus,
                    "mind speaks"
                );

                // Record thoughts to transcript
                room.add_message(&persona.name, &response.thoughts, round);

                // Collect new proposals
                for p in &response.proposals {
                    tracing::info!(
                        persona = %persona.name,
                        kind = %p.kind,
                        text = %truncate(&p.text, 100),
                        "mind proposes"
                    );
                    all_proposals.push((persona.name.clone(), p.clone()));
                }

                // Collect votes
                for (key, vote) in &response.votes {
                    tracing::info!(
                        persona = %persona.name,
                        proposal = %truncate(key, 50),
                        vote = %vote,
                        "mind votes"
                    );
                    all_votes
                        .entry(key.clone())
                        .or_default()
                        .insert(persona.name.clone(), vote.clone());
                }

                if !response.consensus {
                    round_consensus = false;
                }
            }

            // WHY: If all minds signal consensus, we can stop deliberation early.
            if round_consensus {
                tracing::debug!(room_id = %room_id, round = round, "conclave reached consensus");
                if let Some(t) = trace.as_ref() {
                    t.event(
                        "mind:consensus",
                        json!({"room_id": room_id, "round": round, "type": "conclave"}),
                    )
                    .await;
                }
                let decision = self.tally_decision(&all_proposals, &all_votes);
                room.close(decision.clone());
                self.save_conclave(room_id, "consensus", &room.transcript, &decision);
                self.execute_decision(&decision).await;
                return Some(decision);
            }
        }

        // -------------------------------------------------------------------------
        // PHASE 3: TIMEOUT FALLBACK
        // WHY: If max_rounds is reached without consensus, still execute actions
        // with 2/3 votes. This ensures progress even if minds disagree.
        // -------------------------------------------------------------------------
        tracing::warn!(room_id = %room_id, "conclave timed out");
        if let Some(t) = trace.as_ref() {
            t.event("mind:timeout", json!({"room_id": room_id, "type": "conclave"}))
                .await;
        }
        room.timeout();

        let decision = self.tally_decision(&all_proposals, &all_votes);
        self.save_conclave(room_id, "timeout", &room.transcript, &decision);
        if !decision.needs.is_empty() || !decision.wants.is_empty() {
            self.execute_decision(&decision).await;
            return Some(decision);
        }

        None
    }

    /// Convene an operational meeting (autonomy) including GitHub integration.
    ///
    /// WHY separate from conclave: Autonomy mode includes GitHub issue/PR context
    /// and uses different prompt grammar (room_autonomy.md vs room_conclave.md).
    pub async fn autonomy(&self, room_id: &str, wake_mode: WakeMode) -> Option<RoomDecision> {
        self.autonomy_with_trace(room_id, wake_mode, None).await
    }

    /// Autonomy with trace support.
    pub async fn autonomy_with_trace(
        &self,
        room_id: &str,
        wake_mode: WakeMode,
        trace: Option<ConclaveTrace>,
    ) -> Option<RoomDecision> {
        let mut room = Room::autonomy(room_id);

        // Build shared context (autonomy = operational meeting, includes GitHub data if gh plugin enabled)
        let context = self.build_context(wake_mode, RoomType::Autonomy);
        let mut all_proposals: Vec<(String, Proposal)> = Vec::new();
        let mut all_votes: HashMap<String, HashMap<String, String>> = HashMap::new();

        let minds = room.minds.clone();

        for round in 0..room.max_rounds {
            let transcript = self.format_transcript(&room.transcript);
            let proposals = self.format_proposals(&all_proposals, &all_votes);

            if let Some(t) = trace.as_ref() {
                t.event(
                    "mind:round_start",
                    json!({"room_id": room_id, "round": round, "type": "autonomy"}),
                )
                .await;
            }

            let mut round_consensus = true;

            for persona in &minds {
                if let Some(t) = trace.as_ref() {
                    t.event(
                        "mind:query",
                        json!({"room_id": room_id, "round": round, "mind": persona.name}),
                    )
                    .await;
                }
                let Some(response) = self
                    .query_mind_with_grammar(
                        persona,
                        &context,
                        &transcript,
                        &proposals,
                        ROOM_AUTONOMY_GRAMMAR,
                        room_id,
                        round,
                        trace.as_ref(),
                    )
                    .await
                else {
                    round_consensus = false;
                    continue;
                };

                if let Some(t) = trace.as_ref() {
                    t.event(
                        "mind:response",
                        json!({
                            "room_id": room_id,
                            "round": round,
                            "mind": persona.name,
                            "consensus": response.consensus,
                            "proposals": response.proposals.len(),
                        }),
                    )
                    .await;
                }

                tracing::info!(
                    persona = %persona.name,
                    thoughts = %truncate(&response.thoughts, 200),
                    proposals = response.proposals.len(),
                    consensus = response.consensus,
                    "autonomy speaks"
                );

                for proposal in &response.proposals {
                    all_proposals.push((persona.name.clone(), proposal.clone()));
                    tracing::info!(persona = %persona.name, kind = %proposal.kind, text = %truncate(&proposal.text, 80), "autonomy proposes");
                }

                for (key, vote) in &response.votes {
                    all_votes
                        .entry(key.clone())
                        .or_default()
                        .insert(persona.name.clone(), vote.clone());
                }

                room.add_message(
                    &persona.name,
                    serde_json::to_string(&response).unwrap_or_default(),
                    round,
                );

                if !response.consensus {
                    round_consensus = false;
                }
            }

            if round_consensus {
                tracing::debug!(room_id = %room_id, round = round, "autonomy reached consensus");
                if let Some(t) = trace.as_ref() {
                    t.event(
                        "mind:consensus",
                        json!({"room_id": room_id, "round": round, "type": "autonomy"}),
                    )
                    .await;
                }
                let decision = self.tally_decision(&all_proposals, &all_votes);
                room.close(decision.clone());
                self.save_conclave(room_id, "consensus", &room.transcript, &decision);
                self.execute_decision(&decision).await;
                return Some(decision);
            }
        }

        tracing::warn!(room_id = %room_id, "autonomy timed out");
        if let Some(t) = trace.as_ref() {
            t.event("mind:timeout", json!({"room_id": room_id, "type": "autonomy"}))
                .await;
        }
        room.timeout();

        let decision = self.tally_decision(&all_proposals, &all_votes);
        self.save_conclave(room_id, "timeout", &room.transcript, &decision);
        if !decision.needs.is_empty() || !decision.wants.is_empty() {
            self.execute_decision(&decision).await;
            return Some(decision);
        }

        None
    }

// =============================================================================
// PERSISTENCE
// =============================================================================

    /// Save conclave transcript and decision to the history store.
    ///
    /// WHY: Enables replay and analysis of deliberation sessions. The transcript
    /// preserves the full deliberation flow for debugging and auditing.
    fn save_conclave(
        &self,
        room_id: &str,
        status: &str,
        transcript: &[super::room::RoomMessage],
        decision: &RoomDecision,
    ) {
        let transcript_json = serde_json::to_string(transcript).unwrap_or_default();
        let decision_json = serde_json::to_string(decision).unwrap_or_default();

        if let Err(e) = self
            .store
            .save_conclave(room_id, status, &transcript_json, &decision_json)
        {
            tracing::error!(error = %e, "failed to save conclave");
        }
    }

// =============================================================================
// CONTEXT BUILDING
// =============================================================================

    /// Build shared context for all minds in the room.
    ///
    /// WHY: The context provides minds with the current state of the system
    /// (recent activity, LTM, wants pool, GitHub data if autonomy mode). All
    /// minds see the same context to ensure consistent deliberation.
    fn build_context(&self, wake_mode: WakeMode, room_type: RoomType) -> String {
        let bundle_builder = MindBundleBuilder::new(self.store.clone());
        let bundle_cfg = MindBundleConfig::new("conclave", self.scopes.clone())
            .with_wake_mode(wake_mode)
            .with_workspace(self.workspace.clone())
            .with_fever(self.fever.clone())
            .with_filter(self.filter.clone())
            .with_poverty(self.poverty.clone())
            .with_room_type(room_type);
        let messages = bundle_builder.build(&bundle_cfg);

        // Extract both system (which has init/boot instructions) and user (LTM + activity)
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

    fn format_transcript(&self, transcript: &[super::room::RoomMessage]) -> String {
        if transcript.is_empty() {
            return "(no discussion yet)".to_string();
        }

        transcript
            .iter()
            .map(|m| format!("[{}] {}", m.mind, m.content))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn format_proposals(
        &self,
        proposals: &[(String, Proposal)],
        votes: &HashMap<String, HashMap<String, String>>,
    ) -> String {
        if proposals.is_empty() {
            return "(no proposals yet)".to_string();
        }

        proposals
            .iter()
            .map(|(proposer, p)| {
                let key = format!("{}:{}", p.kind, p.text);
                let vote_summary = votes
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
                    p.kind, p.text, p.priority, proposer, vote_summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn query_mind(
        &self,
        persona: &MindPersona,
        context: &str,
        transcript: &str,
        proposals: &str,
        room_id: &str,
        round: usize,
        trace: Option<&ConclaveTrace>,
    ) -> Option<MindResponse> {
        self.query_mind_with_grammar(
            persona,
            context,
            transcript,
            proposals,
            ROOM_CONCLAVE_GRAMMAR,
            room_id,
            round,
            trace,
        )
        .await
    }

    async fn query_mind_with_grammar(
        &self,
        persona: &MindPersona,
        context: &str,
        transcript: &str,
        proposals: &str,
        grammar: &str,
        room_id: &str,
        round: usize,
        trace: Option<&ConclaveTrace>,
    ) -> Option<MindResponse> {
        let mind_cfg = MindConfig::from_config();

        if !mind_cfg.llm.enabled {
            tracing::warn!(persona = %persona.name, "mind LLM not configured, skipping query");
            return None;
        }

        let system = format!("{}\n\n{}", persona.system_prompt, grammar);

        let user_prompt = format!(
            "## Current Context\n\n{}\n\n## Discussion So Far\n\n{}\n\n## Proposals On The Table\n\n{}\n\nRespond with your thoughts, any new proposals, your votes, and whether you believe we have consensus.",
            context, transcript, proposals
        );

        let messages = vec![
            ChatMessage::new(Role::System, system),
            ChatMessage::new(Role::User, user_prompt),
        ];

        if let Some(t) = trace {
            t.event(
                "llm:request",
                json!({
                    "room_id": room_id,
                    "round": round,
                    "mind": persona.name,
                }),
            )
            .await;
        }

        let Some(k) = Kernel::get() else {
            tracing::error!(persona = %persona.name, "kernel not initialized");
            return None;
        };

        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "llm:chat",
            json!({
                "messages": messages,
            }),
        )
        .with_actor(format!("mind/{}", persona.name));

        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let mut content = String::new();
        while let Some(frame) = rx.recv().await {
            match frame.op {
                crate::kernel::FrameOp::Item => {
                    let Some(data) = frame.data.as_ref() else {
                        continue;
                    };
                    match data.get("type").and_then(|v| v.as_str()) {
                        Some("text_delta") => {
                            if let Some(text) = data.get("content").and_then(|v| v.as_str()) {
                                content.push_str(text);
                            }
                        }
                        Some("thinking") => {}
                        _ => {}
                    }
                }
                crate::kernel::FrameOp::Done => break,
                crate::kernel::FrameOp::Error => {
                    let msg = frame
                        .data
                        .as_ref()
                        .and_then(|d| d.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("llm error")
                        .to_string();
                    tracing::error!(persona = %persona.name, error = %msg, "mind query failed");
                    return None;
                }
                _ => {}
            }
        }

        if content.trim().is_empty() {
            tracing::error!(persona = %persona.name, "mind query returned no content");
            return None;
        }

        // Try to parse JSON from the response.
        // The response might have markdown code blocks, so extract JSON.
        let json_str = extract_json(&content);
        match serde_json::from_str::<MindResponse>(&json_str) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(
                    persona = %persona.name,
                    error = %e,
                    content = %content,
                    "failed to parse mind response"
                );
                None
            }
        }
    }

// =============================================================================
// VOTE TALLYING
// =============================================================================

    /// Tally votes and build a RoomDecision with all 2/3-approved proposals.
    ///
    /// WHY 2/3 threshold: Balances consensus with progress. Unanimity would
    /// stall on disagreement; simple majority would be too aggressive.
    ///
    /// WHY proposer implicit yes: The mind proposing an action implicitly votes
    /// yes for it, so we count their vote even if they didn't explicitly vote.
    fn tally_decision(
        &self,
        proposals: &[(String, Proposal)],
        votes: &HashMap<String, HashMap<String, String>>,
    ) -> RoomDecision {
        let mut decision = RoomDecision::default();

        for (proposer, p) in proposals {
            let key = format!("{}:{}", p.kind, p.text);
            let vote_map = votes.get(&key);

            // Count yes votes
            let yes_count = vote_map
                .map(|v| v.values().filter(|&vote| vote == "yes").count())
                .unwrap_or(0);

            // Proposer implicitly votes yes
            let total_yes = if vote_map.map(|v| v.contains_key(proposer)).unwrap_or(false) {
                yes_count
            } else {
                yes_count + 1
            };

            // 2/3 threshold (2 out of 3)
            if total_yes >= 2 {
                match p.kind.as_str() {
                    "need" => {
                        let priority = if p.priority.is_empty() {
                            "normal".to_string()
                        } else {
                            p.priority.clone()
                        };
                        let reconvene = priority == "urgent";
                        decision.needs.push(NeedProposal {
                            need: p.text.clone(),
                            context: p.context.clone(),
                            priority,
                            reconvene,
                            votes: vote_map
                                .map(|v| {
                                    v.iter()
                                        .filter(|(_, vote)| *vote == "yes")
                                        .map(|(m, _)| m.clone())
                                        .collect()
                                })
                                .unwrap_or_default(),
                        });
                    }
                    "want" => {
                        decision.wants.push(WantProposal {
                            want: p.text.clone(),
                            context: p.context.clone(),
                            priority: if p.priority.is_empty() {
                                "normal".to_string()
                            } else {
                                p.priority.clone()
                            },
                            proposer: proposer.clone(),
                        });
                    }
                    "ltm" => {
                        // text = operation (append/replace/remove)
                        // content = what to add/replace with
                        // pattern = what to find (for replace/remove)
                        decision.ltm_ops.push(LtmProposal {
                            kind: p.text.clone(),
                            content: p.content.clone(),
                            pattern: p.pattern.clone(),
                            proposer: proposer.clone(),
                        });
                    }
                    "self" => {
                        // text = operation (append/replace/remove)
                        // content = what to add/replace with
                        // pattern = what to find (for replace/remove)
                        decision.self_ops.push(SelfProposal {
                            kind: p.text.clone(),
                            content: p.content.clone(),
                            pattern: p.pattern.clone(),
                            proposer: proposer.clone(),
                        });
                    }
                    "control" => {
                        let mode = if !p.mode.is_empty() {
                            p.mode.clone()
                        } else if !p.priority.is_empty() {
                            p.priority.clone()
                        } else {
                            "hard".to_string()
                        };

                        decision.control_ops.push(ControlProposal {
                            kind: p.text.clone(),
                            mode,
                            reason: p.context.clone(),
                            proposer: proposer.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }

        decision
    }

// =============================================================================
// DECISION EXECUTION
// =============================================================================

    /// Execute a RoomDecision by creating needs, wants, and applying LTM/self ops.
    ///
    /// WHY: This is where deliberation translates into action. Needs are enqueued
    /// for heads to process, wants are stored for future deliberation, LTM/self
    /// ops update the system's memory and identity.
    async fn execute_decision(&self, decision: &RoomDecision) {
        // Create needs
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
                        "source": "conclave",
                        "priority": priority,
                        "need": need.need.clone(),
                        "context": need.context.clone(),
                        "scope": "main",
                        "reconvene": need.reconvene,
                    }),
                )
                .with_actor("system/conclave");

                let mut rx = dispatcher.dispatch(
                    req,
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                    tokio_util::sync::CancellationToken::new(),
                );
                let _ = rx.recv().await;
            }

            tracing::info!(need = %need.need, priority = %priority, "mind proposes");
        }

        // Create wants
        for want in &decision.wants {
            let want_id = uuid::Uuid::new_v4().to_string();
            if let Err(e) = self.store.add_want(
                &want_id,
                &want.want,
                &want.context,
                &want.priority,
                "conclave",
            ) {
                tracing::error!(error = %e, "failed to add want");
            } else {
                tracing::debug!(
                    want = %want.want,
                    "want created"
                );
            }
        }

        // Apply LTM operations
        self.apply_ltm_ops(&decision.ltm_ops);

        // Apply Self operations
        self.apply_self_ops(&decision.self_ops);

        // Apply Control operations
        self.apply_control_ops(&decision.control_ops).await;
    }

    async fn apply_control_ops(&self, ops: &[ControlProposal]) {
        if ops.is_empty() {
            return;
        }

        for op in ops {
            if op.kind != "reboot_collective" {
                continue;
            }
            let mode = if op.mode.is_empty() {
                "hard"
            } else {
                op.mode.as_str()
            };
            let reason = if op.reason.trim().is_empty() {
                "requested by conclave"
            } else {
                op.reason.trim()
            };

            let epoch = bump_reboot_epoch();

            tracing::warn!(epoch, mode, proposer = %op.proposer, reason = %reason, "reboot requested");

            // One reboot per decision is sufficient.
            break;
        }
    }

    /// Apply LTM operations (append/replace/remove) to mind/memory.md.
    ///
    /// WHY file-based: LTM is stored in workspace/mind/memory.md for human
    /// editability and version control. This replaced the legacy DB storage.
    fn apply_ltm_ops(&self, ops: &[LtmProposal]) {
        if ops.is_empty() {
            return;
        }

        let path = workspace_mind_memory(&self.workspace);
        if !path.parent().map(|p| p.exists()).unwrap_or(false) {
            tracing::warn!("cannot resolve mind/memory.md for workspace");
            return;
        };

        let current = match read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // One-time migration from legacy DB location.
                let legacy = self.store.get_head_ltm("conclave").unwrap_or_default();
                if !legacy.trim().is_empty() {
                    let _ = atomic_write_file_0600(&path, legacy.trim());
                    legacy
                } else {
                    String::new()
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to read mind/memory.md");
                String::new()
            }
        };

        let mut ltm = current.clone();

        for op in ops {
            match op.kind.as_str() {
                "append" => {
                    let content = op.content.trim();
                    if content.is_empty() {
                        continue;
                    }
                    if !ltm.is_empty() {
                        ltm.push_str("\n\n");
                    }
                    ltm.push_str(content);
                    tracing::info!(
                        content = %truncate(content, 100),
                        proposer = %op.proposer,
                        "LTM append"
                    );
                }
                "replace" => {
                    if op.pattern.is_empty() {
                        continue;
                    }
                    if let Some(pos) = ltm.find(&op.pattern) {
                        let end = pos + op.pattern.len();
                        ltm.replace_range(pos..end, &op.content);
                        tracing::info!(
                            pattern = %truncate(&op.pattern, 50),
                            proposer = %op.proposer,
                            "LTM replace"
                        );
                    }
                }
                "remove" => {
                    if op.pattern.is_empty() {
                        continue;
                    }
                    if ltm.contains(&op.pattern) {
                        ltm = ltm.replace(&op.pattern, "");
                        while ltm.contains("\n\n\n") {
                            ltm = ltm.replace("\n\n\n", "\n\n");
                        }
                        ltm = ltm.trim().to_string();
                        tracing::info!(
                            pattern = %truncate(&op.pattern, 50),
                            proposer = %op.proposer,
                            "LTM remove"
                        );
                    }
                }
                _ => {}
            }
        }

        if ltm != current {
            if let Err(e) = atomic_write_file_0600(&path, &ltm) {
                tracing::error!(error = %e, "failed to save mind/memory.md");
            } else {
                tracing::info!(ltm_len = ltm.len(), "LTM updated");
            }
        }
    }

    /// Apply Self operations (append/replace/remove) to mind/self.md.
    ///
    /// WHY: Self is the system's evolving identity document. Minds can modify
    /// it through deliberation to update their understanding of purpose, values,
    /// or constraints.
    fn apply_self_ops(&self, ops: &[SelfProposal]) {
        if ops.is_empty() {
            return;
        }

        let path = workspace_mind_self(&self.workspace);
        if !path.parent().map(|p| p.exists()).unwrap_or(false) {
            tracing::warn!("cannot resolve mind/self.md for workspace");
            return;
        };

        let current = match read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // One-time migration from legacy DB location.
                let legacy = self.store.get_conclave_self().unwrap_or_default();
                if !legacy.trim().is_empty() {
                    let _ = atomic_write_file_0600(&path, legacy.trim());
                    legacy
                } else {
                    String::new()
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to read mind/self.md");
                String::new()
            }
        };

        let mut identity = current.clone();

        for op in ops {
            match op.kind.as_str() {
                "append" => {
                    let content = op.content.trim();
                    if content.is_empty() {
                        continue;
                    }
                    if !identity.is_empty() {
                        identity.push_str("\n\n");
                    }
                    identity.push_str(content);
                    tracing::info!(
                        content = %truncate(content, 100),
                        proposer = %op.proposer,
                        "Self append"
                    );
                }
                "replace" => {
                    if op.pattern.is_empty() {
                        continue;
                    }
                    if let Some(pos) = identity.find(&op.pattern) {
                        let end = pos + op.pattern.len();
                        identity.replace_range(pos..end, &op.content);
                        tracing::info!(
                            pattern = %truncate(&op.pattern, 50),
                            proposer = %op.proposer,
                            "Self replace"
                        );
                    }
                }
                "remove" => {
                    if op.pattern.is_empty() {
                        continue;
                    }
                    if identity.contains(&op.pattern) {
                        identity = identity.replace(&op.pattern, "");
                        while identity.contains("\n\n\n") {
                            identity = identity.replace("\n\n\n", "\n\n");
                        }
                        identity = identity.trim().to_string();
                        tracing::info!(
                            pattern = %truncate(&op.pattern, 50),
                            proposer = %op.proposer,
                            "Self remove"
                        );
                    }
                }
                _ => {}
            }
        }

        if identity != current {
            if let Err(e) = atomic_write_file_0600(&path, &identity) {
                tracing::error!(error = %e, "failed to save mind/self.md");
            } else {
                tracing::info!(self_len = identity.len(), "Self updated");
            }
        }
    }
}

// =============================================================================
// HELPERS
// =============================================================================

/// Truncate a string for logging (replaces newlines with spaces).
///
/// WHY: Preserves readability in single-line log output.
fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        return s;
    }
    let clipped: String = s.chars().take(max).collect();
    format!("{}...", clipped)
}

/// Extract JSON from LLM response (handles markdown code blocks).
///
/// WHY: LLMs often wrap JSON in ```json...``` blocks. This parser handles
/// both raw JSON and code-fenced JSON for robust parsing.
fn extract_json(content: &str) -> String {
    // Try to find JSON in code blocks first
    if let Some(start) = content.find("```json") {
        if let Some(end) = content[start..]
            .find("```\n")
            .or_else(|| content[start..].rfind("```"))
        {
            let json_start = start + 7; // skip ```json
            let json_end = start + end;
            if json_end > json_start {
                return content[json_start..json_end].trim().to_string();
            }
        }
    }

    // Try to find raw JSON object
    if let Some(start) = content.find('{') {
        if let Some(end) = content.rfind('}') {
            if end > start {
                return content[start..=end].to_string();
            }
        }
    }

    content.to_string()
}
