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

use crate::bus::{NeedPriority, Origin, Scope, respond};
use crate::history::Store;
use crate::llm::{ChatMessage, OpenAICompatClient, Role};

use super::room::{Room, RoomDecision, MindPersona, NeedProposal, WantProposal};
use super::mind_bundle::WakeMode;
use super::{RuntimeBus, MindBundleBuilder, MindBundleConfig, MindConfig};

const ROOM_GRAMMAR: &str = include_str!("room_grammar.md");

pub struct Conclave {
    bus: RuntimeBus,
    store: Arc<Store>,
    scopes: Vec<Scope>,
    workspace: PathBuf,
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
}

impl Conclave {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, scopes: Vec<Scope>, workspace: PathBuf) -> Self {
        Self { bus, store, scopes, workspace }
    }

    pub async fn convene(&self, room_id: &str, wake_mode: WakeMode) -> Option<RoomDecision> {
        let mut room = Room::conclave(room_id);

        // Build shared context
        let context = self.build_context(wake_mode);

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

                let response = match self.query_mind(persona, &context, &transcript_so_far, &proposals_summary).await {
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
                self.execute_decision(&decision).await;
                return Some(decision);
            }
        }

        tracing::warn!(room_id = %room_id, "conclave timed out");
        room.timeout();

        // Even on timeout, execute anything with 2/3 votes
        let decision = self.tally_decision(&all_proposals, &all_votes);
        if !decision.needs.is_empty() || !decision.wants.is_empty() {
            self.execute_decision(&decision).await;
            return Some(decision);
        }

        None
    }

    fn build_context(&self, wake_mode: WakeMode) -> String {
        let bundle_builder = MindBundleBuilder::new(self.store.clone());
        let bundle_cfg = MindBundleConfig::new("conclave", self.scopes.clone())
            .with_wake_mode(wake_mode)
            .with_workspace(self.workspace.clone());
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
        let mind_cfg = MindConfig::from_env();

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

        let system = format!("{}\n\n{}", persona.system_prompt, ROOM_GRAMMAR);

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
                        let priority = if p.priority.is_empty() { "normal".to_string() } else { p.priority.clone() };
                        let reconvene = priority == "urgent";
                        decision.needs.push(NeedProposal {
                            need: p.text.clone(),
                            context: p.context.clone(),
                            priority,
                            reconvene,
                            votes: vote_map
                                .map(|v| v.iter().filter(|(_, vote)| *vote == "yes").map(|(m, _)| m.clone()).collect())
                                .unwrap_or_default(),
                        });
                    }
                    "want" => {
                        decision.wants.push(WantProposal {
                            want: p.text.clone(),
                            context: p.context.clone(),
                            priority: if p.priority.is_empty() { "normal".to_string() } else { p.priority.clone() },
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
                "low" => NeedPriority::Low,
                "high" => NeedPriority::High,
                "urgent" => NeedPriority::Urgent,
                _ => NeedPriority::Normal,
            };

            let msg = respond::need_request_with_reconvene(
                "conclave",
                Scope::from("@need_service"),
                &need_id,
                "conclave",
                priority,
                &need.need,
                &need.context,
                need.reconvene,
            )
            .with_origin(Origin::System);

            self.bus.publish(msg).await;

            tracing::info!(
                need = %need.need,
                priority = ?priority,
                "mind proposes"
            );
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
        if let Some(end) = content[start..].find("```\n").or_else(|| content[start..].rfind("```")) {
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
