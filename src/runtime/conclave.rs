// Conclave: deliberation loop where MindManager, HeadManager, HandManager reach consensus.
//
// On each mind tick, the conclave convenes:
// 1. Build context (recent activity, LTM, wants pool)
// 2. Each Mind responds with proposals and votes
// 3. Iterate until consensus or max_rounds
// 4. Execute agreed needs/wants/LTM ops

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Scope;
use crate::history::Store;
use crate::kernel::Frame;
use crate::llm::{ChatMessage, OpenAICompatClient, Role};
use crate::runtime::Kernel;
use crate::runtime::bump_reboot_epoch;
use crate::runtime::{
    atomic_write_file_0600, read_optional_file, workspace_mind_memory, workspace_mind_self,
};

use super::mind_bundle::{FeverMode, RoomType, WakeMode};
use super::TactMode;
use super::room::{
    ControlProposal, LtmProposal, MindPersona, NeedProposal, Room, RoomDecision, SelfProposal,
    WantProposal,
};
use super::{MindBundleBuilder, MindBundleConfig, MindConfig};

const ROOM_CONCLAVE_GRAMMAR: &str = include_str!("room_conclave.md");
const ROOM_AUTONOMY_GRAMMAR: &str = include_str!("room_autonomy.md");

pub struct Conclave {
    store: Arc<Store>,
    scopes: Vec<Scope>,
    workspace: PathBuf,
    fever: FeverMode,
    tact: TactMode,
    poverty: super::PovertyMode,
}

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

impl Conclave {
    pub fn new(store: Arc<Store>, scopes: Vec<Scope>, workspace: PathBuf) -> Self {
        let mind_cfg = MindConfig::from_config();
        Self {
            store,
            scopes,
            workspace,
            fever: mind_cfg.fever,
            tact: mind_cfg.tact,
            poverty: mind_cfg.poverty,
        }
    }

    pub fn with_fever(mut self, fever: FeverMode) -> Self {
        self.fever = fever;
        self
    }

    pub fn with_tact(mut self, tact: TactMode) -> Self {
        self.tact = tact;
        self
    }

    pub fn with_poverty(mut self, poverty: super::PovertyMode) -> Self {
        self.poverty = poverty;
        self
    }

    pub async fn convene(&self, room_id: &str, wake_mode: WakeMode) -> Option<RoomDecision> {
        let mut room = Room::conclave(room_id);

        // Build shared context (conclave = strategic meeting)
        let context = self.build_context(wake_mode, RoomType::Conclave);

        // Track all proposals and votes
        let mut all_proposals: Vec<(String, Proposal)> = Vec::new(); // (proposer, proposal)
        let mut all_votes: HashMap<String, HashMap<String, String>> = HashMap::new(); // proposal_key -> mind -> vote

        // Clone minds to avoid borrow issues
        let minds = room.minds.clone();

        for round in 0..room.max_rounds {
            tracing::debug!(room_id = %room_id, round = round, "conclave round");

            let mut round_consensus = true;

            for persona in &minds {
                let transcript_so_far = self.format_transcript(&room.transcript);
                let proposals_summary = self.format_proposals(&all_proposals, &all_votes);

                let response = match self
                    .query_mind(persona, &context, &transcript_so_far, &proposals_summary)
                    .await
                {
                    Some(r) => r,
                    None => continue,
                };

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

            // Check if all minds said consensus
            if round_consensus {
                tracing::debug!(room_id = %room_id, round = round, "conclave reached consensus");
                let decision = self.tally_decision(&all_proposals, &all_votes);
                room.close(decision.clone());
                self.save_conclave(room_id, "consensus", &room.transcript, &decision);
                self.execute_decision(&decision).await;
                return Some(decision);
            }
        }

        tracing::warn!(room_id = %room_id, "conclave timed out");
        room.timeout();

        // Even on timeout, execute anything with 2/3 votes
        let decision = self.tally_decision(&all_proposals, &all_votes);
        self.save_conclave(room_id, "timeout", &room.transcript, &decision);
        if !decision.needs.is_empty() || !decision.wants.is_empty() {
            self.execute_decision(&decision).await;
            return Some(decision);
        }

        None
    }

    pub async fn autonomy(&self, room_id: &str, wake_mode: WakeMode) -> Option<RoomDecision> {
        let mut room = Room::autonomy(room_id);

        // Build shared context (autonomy = operational meeting, includes GitHub data if gh plugin enabled)
        let context = self.build_context(wake_mode, RoomType::Autonomy);
        let mut all_proposals: Vec<(String, Proposal)> = Vec::new();
        let mut all_votes: HashMap<String, HashMap<String, String>> = HashMap::new();

        let minds = room.minds.clone();

        for round in 0..room.max_rounds {
            let transcript = self.format_transcript(&room.transcript);
            let proposals = self.format_proposals(&all_proposals, &all_votes);

            let mut round_consensus = true;

            for persona in &minds {
                let Some(response) = self
                    .query_mind_with_grammar(
                        persona,
                        &context,
                        &transcript,
                        &proposals,
                        ROOM_AUTONOMY_GRAMMAR,
                    )
                    .await
                else {
                    round_consensus = false;
                    continue;
                };

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
                let decision = self.tally_decision(&all_proposals, &all_votes);
                room.close(decision.clone());
                self.save_conclave(room_id, "consensus", &room.transcript, &decision);
                self.execute_decision(&decision).await;
                return Some(decision);
            }
        }

        tracing::warn!(room_id = %room_id, "autonomy timed out");
        room.timeout();

        let decision = self.tally_decision(&all_proposals, &all_votes);
        self.save_conclave(room_id, "timeout", &room.transcript, &decision);
        if !decision.needs.is_empty() || !decision.wants.is_empty() {
            self.execute_decision(&decision).await;
            return Some(decision);
        }

        None
    }

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

    fn build_context(&self, wake_mode: WakeMode, room_type: RoomType) -> String {
        let bundle_builder = MindBundleBuilder::new(self.store.clone());
        let bundle_cfg = MindBundleConfig::new("conclave", self.scopes.clone())
            .with_wake_mode(wake_mode)
            .with_workspace(self.workspace.clone())
            .with_fever(self.fever.clone())
            .with_tact(self.tact.clone())
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
    ) -> Option<MindResponse> {
        self.query_mind_with_grammar(
            persona,
            context,
            transcript,
            proposals,
            ROOM_CONCLAVE_GRAMMAR,
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
    ) -> Option<MindResponse> {
        let mind_cfg = MindConfig::from_config();

        if !mind_cfg.llm.enabled {
            tracing::warn!(persona = %persona.name, "mind LLM not configured, skipping query");
            return None;
        }

        let client = OpenAICompatClient::new(
            &mind_cfg.llm.base_url,
            &mind_cfg.llm.api_key,
            &mind_cfg.llm.model,
            mind_cfg.llm.temperature.or(Some(persona.temperature)),
            mind_cfg.llm.max_tokens.or(Some(1000)),
            mind_cfg.llm.extra_headers.clone(),
        );

        let system = format!("{}\n\n{}", persona.system_prompt, grammar);

        let user_prompt = format!(
            "## Current Context\n\n{}\n\n## Discussion So Far\n\n{}\n\n## Proposals On The Table\n\n{}\n\nRespond with your thoughts, any new proposals, your votes, and whether you believe we have consensus.",
            context, transcript, proposals
        );

        let messages = vec![
            ChatMessage::new(Role::System, system),
            ChatMessage::new(Role::User, user_prompt),
        ];

        match client.chat(messages).await {
            Ok(response) => {
                let content = response.content;

                // Try to parse JSON from the response
                // The response might have markdown code blocks, so extract JSON
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
            Err(e) => {
                tracing::error!(persona = %persona.name, error = %e, "mind query failed");
                None
            }
        }
    }

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

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        return s;
    }
    let clipped: String = s.chars().take(max).collect();
    format!("{}...", clipped)
}

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
