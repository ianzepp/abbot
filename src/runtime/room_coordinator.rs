// RoomCoordinator: event-driven orchestration for multi-mind deliberation.
//
// STATUS: Work in progress. Core coordinator logic is implemented and tested,
// but not yet integrated into the runtime.
//
// ASPIRATION: Replace the synchronous Conclave implementation with an event-driven
// architecture where minds are HeadService-like instances on the bus rather than
// direct LLM calls in a tight loop.
//
// Current Conclave architecture:
// - MindService triggers Conclave on events (idle, boot, etc.)
// - Conclave.convene() runs a synchronous loop querying each mind persona directly
// - Minds are just LLM calls, not bus participants
// - No tool access, no async work between rounds
//
// Target architecture:
// - RoomCoordinator announces rounds via bus events
// - MindHead services (new) subscribe to room scopes and respond to room_round events
// - Coordinator implements barrier logic (wait for N responses or timeout)
// - Minds become full participants that could use tools if needed
// - MindService shrinks to just trigger logic, or merges into harness
//
// Event protocol:
// - room_start: coordinator announces room with context and participants
// - room_round: coordinator announces round N, includes current proposals/votes
// - mind_spoke: participant responds with proposals, votes, consensus flag
// - room_end: coordinator announces result with final decision
//
// Next steps to complete integration:
// 1. Create RuntimeBus adapter implementing RoomBus trait
// 2. Create MindHead service that responds to room_round events
// 3. Wire RoomCoordinator into MindService to replace Conclave calls
// 4. Deprecate and remove synchronous Conclave implementation

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::timeout;

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope, respond};

use super::room::{
    ControlProposal, LtmProposal, NeedProposal, RoomDecision, RoomKind, SelfProposal, WantProposal,
};

const DEFAULT_MAX_ROUNDS: usize = 5;
const DEFAULT_ROUND_TIMEOUT_SECS: u64 = 60;

// Response from a mind for a single round
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MindResponse {
    pub mind_id: String,
    pub round: usize,
    #[serde(default)]
    pub thoughts: String,
    #[serde(default)]
    pub proposals: Vec<Proposal>,
    #[serde(default)]
    pub votes: HashMap<String, Vote>,
    #[serde(default)]
    pub consensus: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: ProposalKind,
    pub text: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalKind {
    Need,
    Want,
    Ltm,
    #[serde(rename = "self")]
    SelfOp,
    Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vote {
    Yes,
    No,
    Abstain,
}

// Collected state for a room session
#[derive(Debug, Default)]
struct RoomState {
    proposals: Vec<(String, Proposal)>,
    votes: HashMap<String, HashMap<String, Vote>>,
    transcript: Vec<MindResponse>,
}

// Result of collecting responses for a round
enum CollectResult {
    Complete(Vec<MindResponse>),
    Timeout {
        responses: Vec<MindResponse>,
        missing: Vec<String>,
    },
}

// Test-friendly interface for the bus
#[async_trait::async_trait]
pub trait RoomBus: Send + Sync {
    async fn publish(&self, msg: Message);
    async fn recv(&mut self) -> Option<Message>;
}

pub struct RoomCoordinator<B: RoomBus> {
    bus: B,
    room_id: String,
    room_kind: RoomKind,
    room_scope: Scope,
    participants: Vec<String>,
    max_rounds: usize,
    round_timeout: Duration,
}

impl<B: RoomBus> RoomCoordinator<B> {
    pub fn new(bus: B, room_id: impl Into<String>, room_kind: RoomKind) -> Self {
        let room_id = room_id.into();
        let room_scope = Scope::from(format!("@room:{}", room_id));
        Self {
            bus,
            room_id,
            room_kind,
            room_scope,
            participants: Vec::new(),
            max_rounds: DEFAULT_MAX_ROUNDS,
            round_timeout: Duration::from_secs(DEFAULT_ROUND_TIMEOUT_SECS),
        }
    }

    pub fn with_participants(mut self, participants: Vec<String>) -> Self {
        self.participants = participants;
        self
    }

    pub fn with_max_rounds(mut self, max: usize) -> Self {
        self.max_rounds = max;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.round_timeout = timeout;
        self
    }

    pub async fn run(mut self, context: &str) -> RoomResult {
        if self.participants.is_empty() {
            return RoomResult::Error("no participants".into());
        }

        let mut state = RoomState::default();

        self.emit_room_start(context).await;

        for round in 1..=self.max_rounds {
            self.emit_room_round(round, &state).await;

            match self.collect_responses(round).await {
                CollectResult::Complete(responses) => {
                    let all_consensus = self.process_responses(&mut state, responses);
                    if all_consensus {
                        let decision = self.tally_decision(&state);
                        self.emit_room_end("consensus", &decision).await;
                        return RoomResult::Consensus(decision);
                    }
                }
                CollectResult::Timeout { responses, missing } => {
                    self.process_responses(&mut state, responses);
                    let decision = self.tally_decision(&state);
                    self.emit_room_end("timeout", &decision).await;
                    return RoomResult::Timeout {
                        round,
                        missing,
                        decision,
                    };
                }
            }
        }

        let decision = self.tally_decision(&state);
        self.emit_room_end("max_rounds", &decision).await;
        RoomResult::MaxRounds(decision)
    }

    async fn emit_room_start(&self, context: &str) {
        let msg = respond::event(
            "room_coordinator",
            self.room_scope.clone(),
            "room_start",
            json!({
                "room_id": self.room_id,
                "room_kind": format!("{:?}", self.room_kind),
                "participants": self.participants,
                "context": context,
            }),
        )
        .with_origin(Origin::System);
        self.bus.publish(msg).await;
    }

    async fn emit_room_round(&self, round: usize, state: &RoomState) {
        let proposals_summary: Vec<_> = state
            .proposals
            .iter()
            .map(|(proposer, p)| {
                let votes = state.votes.get(&p.id).cloned().unwrap_or_default();
                json!({
                    "id": p.id,
                    "kind": p.kind,
                    "text": p.text,
                    "proposer": proposer,
                    "votes": votes,
                })
            })
            .collect();

        let msg = respond::event(
            "room_coordinator",
            self.room_scope.clone(),
            "room_round",
            json!({
                "room_id": self.room_id,
                "round": round,
                "max_rounds": self.max_rounds,
                "proposals": proposals_summary,
            }),
        )
        .with_origin(Origin::System);
        self.bus.publish(msg).await;
    }

    async fn emit_room_end(&self, status: &str, decision: &RoomDecision) {
        let msg = respond::event(
            "room_coordinator",
            self.room_scope.clone(),
            "room_end",
            json!({
                "room_id": self.room_id,
                "status": status,
                "decision": decision,
            }),
        )
        .with_origin(Origin::System);
        self.bus.publish(msg).await;
    }

    async fn collect_responses(&mut self, round: usize) -> CollectResult {
        let mut responses = Vec::new();
        let mut remaining: Vec<String> = self.participants.clone();

        let deadline = timeout(self.round_timeout, async {
            loop {
                if remaining.is_empty() {
                    break;
                }

                let Some(msg) = self.bus.recv().await else {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                };

                if let Some(response) = self.parse_mind_spoke(&msg, round) {
                    if remaining.contains(&response.mind_id) {
                        remaining.retain(|id| id != &response.mind_id);
                        responses.push(response);
                    }
                }
            }
        });

        match deadline.await {
            Ok(_) => CollectResult::Complete(responses),
            Err(_) => CollectResult::Timeout {
                responses,
                missing: remaining,
            },
        }
    }

    fn parse_mind_spoke(&self, msg: &Message, expected_round: usize) -> Option<MindResponse> {
        if msg.op != MessageOp::Event {
            return None;
        }

        let MessageData::Event { kind, payload } = &msg.data else {
            return None;
        };

        if kind != "mind_spoke" {
            return None;
        }

        let room_id = payload.get("room_id")?.as_str()?;
        if room_id != self.room_id {
            return None;
        }

        let round = payload.get("round")?.as_u64()? as usize;
        if round != expected_round {
            return None;
        }

        serde_json::from_value(payload.clone()).ok()
    }

    fn process_responses(&self, state: &mut RoomState, responses: Vec<MindResponse>) -> bool {
        let mut all_consensus = true;

        for response in responses {
            if !response.consensus {
                all_consensus = false;
            }

            for proposal in &response.proposals {
                state
                    .proposals
                    .push((response.mind_id.clone(), proposal.clone()));
            }

            for (proposal_id, vote) in &response.votes {
                state
                    .votes
                    .entry(proposal_id.clone())
                    .or_default()
                    .insert(response.mind_id.clone(), *vote);
            }

            state.transcript.push(response);
        }

        all_consensus && !state.transcript.is_empty()
    }

    fn tally_decision(&self, state: &RoomState) -> RoomDecision {
        let mut decision = RoomDecision::default();
        let threshold = (self.participants.len() * 2 + 2) / 3;

        for (proposer, proposal) in &state.proposals {
            let votes = state.votes.get(&proposal.id);

            let yes_count = votes
                .map(|v| v.values().filter(|&&vote| vote == Vote::Yes).count())
                .unwrap_or(0);

            let proposer_voted = votes.map(|v| v.contains_key(proposer)).unwrap_or(false);
            let total_yes = if proposer_voted {
                yes_count
            } else {
                yes_count + 1
            };

            if total_yes < threshold {
                continue;
            }

            let voters: Vec<String> = votes
                .map(|v| {
                    v.iter()
                        .filter(|&(_, vote)| *vote == Vote::Yes)
                        .map(|(m, _)| m.clone())
                        .collect()
                })
                .unwrap_or_default();

            match proposal.kind {
                ProposalKind::Need => {
                    let priority = if proposal.priority.is_empty() {
                        "normal".to_string()
                    } else {
                        proposal.priority.clone()
                    };
                    decision.needs.push(NeedProposal {
                        need: proposal.text.clone(),
                        context: proposal.context.clone(),
                        priority: priority.clone(),
                        reconvene: priority == "urgent",
                        votes: voters,
                    });
                }
                ProposalKind::Want => {
                    decision.wants.push(WantProposal {
                        want: proposal.text.clone(),
                        context: proposal.context.clone(),
                        priority: if proposal.priority.is_empty() {
                            "normal".to_string()
                        } else {
                            proposal.priority.clone()
                        },
                        proposer: proposer.clone(),
                    });
                }
                ProposalKind::Ltm => {
                    decision.ltm_ops.push(LtmProposal {
                        kind: proposal.text.clone(),
                        content: proposal.content.clone(),
                        pattern: proposal.pattern.clone(),
                        proposer: proposer.clone(),
                    });
                }
                ProposalKind::SelfOp => {
                    decision.self_ops.push(SelfProposal {
                        kind: proposal.text.clone(),
                        content: proposal.content.clone(),
                        pattern: proposal.pattern.clone(),
                        proposer: proposer.clone(),
                    });
                }
                ProposalKind::Control => {
                    decision.control_ops.push(ControlProposal {
                        kind: proposal.text.clone(),
                        mode: if proposal.mode.is_empty() {
                            "hard".to_string()
                        } else {
                            proposal.mode.clone()
                        },
                        reason: proposal.context.clone(),
                        proposer: proposer.clone(),
                    });
                }
            }
        }

        decision
    }
}

#[derive(Debug)]
pub enum RoomResult {
    Consensus(RoomDecision),
    MaxRounds(RoomDecision),
    Timeout {
        round: usize,
        missing: Vec<String>,
        decision: RoomDecision,
    },
    Error(String),
}

impl RoomResult {
    pub fn decision(&self) -> Option<&RoomDecision> {
        match self {
            RoomResult::Consensus(d) => Some(d),
            RoomResult::MaxRounds(d) => Some(d),
            RoomResult::Timeout { decision, .. } => Some(decision),
            RoomResult::Error(_) => None,
        }
    }

    pub fn is_consensus(&self) -> bool {
        matches!(self, RoomResult::Consensus(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    struct MockBus {
        published: Arc<Mutex<Vec<Message>>>,
        to_receive: Arc<Mutex<VecDeque<Message>>>,
    }

    impl MockBus {
        fn new() -> Self {
            Self {
                published: Arc::new(Mutex::new(Vec::new())),
                to_receive: Arc::new(Mutex::new(VecDeque::new())),
            }
        }

        fn queue_response(&self, msg: Message) {
            self.to_receive.lock().unwrap().push_back(msg);
        }
    }

    #[async_trait::async_trait]
    impl RoomBus for MockBus {
        async fn publish(&self, msg: Message) {
            self.published.lock().unwrap().push(msg);
        }

        async fn recv(&mut self) -> Option<Message> {
            self.to_receive.lock().unwrap().pop_front()
        }
    }

    fn mind_spoke(
        room_id: &str,
        mind_id: &str,
        round: usize,
        consensus: bool,
        proposals: Vec<Proposal>,
        votes: HashMap<String, Vote>,
    ) -> Message {
        respond::event(
            mind_id,
            Scope::from(format!("@room:{}", room_id)),
            "mind_spoke",
            json!({
                "room_id": room_id,
                "mind_id": mind_id,
                "round": round,
                "thoughts": "test thoughts",
                "proposals": proposals,
                "votes": votes,
                "consensus": consensus,
            }),
        )
    }

    fn make_proposal(id: &str, kind: ProposalKind, text: &str) -> Proposal {
        Proposal {
            id: id.to_string(),
            kind,
            text: text.to_string(),
            context: String::new(),
            priority: "normal".to_string(),
            mode: String::new(),
            content: String::new(),
            pattern: String::new(),
        }
    }

    #[tokio::test]
    async fn test_empty_participants_returns_error() {
        let bus = MockBus::new();
        let coord = RoomCoordinator::new(bus, "test-room", RoomKind::Conclave);

        let result = coord.run("context").await;
        assert!(matches!(result, RoomResult::Error(_)));
    }

    #[tokio::test]
    async fn test_consensus_round_one() {
        let bus = MockBus::new();
        let room_id = "test-consensus";

        let proposal = make_proposal("p1", ProposalKind::Need, "do something");

        let mut votes1 = HashMap::new();
        votes1.insert("p1".to_string(), Vote::Yes);

        let mut votes2 = HashMap::new();
        votes2.insert("p1".to_string(), Vote::Yes);

        let mut votes3 = HashMap::new();
        votes3.insert("p1".to_string(), Vote::Yes);

        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            1,
            true,
            vec![proposal],
            votes1,
        ));
        bus.queue_response(mind_spoke(room_id, "mind2", 1, true, vec![], votes2));
        bus.queue_response(mind_spoke(room_id, "mind3", 1, true, vec![], votes3));

        let coord = RoomCoordinator::new(bus, room_id, RoomKind::Conclave)
            .with_participants(vec!["mind1".into(), "mind2".into(), "mind3".into()])
            .with_timeout(Duration::from_millis(100));

        let result = coord.run("test context").await;

        assert!(result.is_consensus());
        let decision = result.decision().unwrap();
        assert_eq!(decision.needs.len(), 1);
        assert_eq!(decision.needs[0].need, "do something");
    }

    #[tokio::test]
    async fn test_no_consensus_needs_multiple_rounds() {
        let bus = MockBus::new();
        let room_id = "test-no-consensus";

        let proposal = make_proposal("p1", ProposalKind::Need, "disputed thing");

        let mut yes_vote = HashMap::new();
        yes_vote.insert("p1".to_string(), Vote::Yes);

        let mut no_vote = HashMap::new();
        no_vote.insert("p1".to_string(), Vote::No);

        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            1,
            false,
            vec![proposal.clone()],
            yes_vote.clone(),
        ));
        bus.queue_response(mind_spoke(
            room_id,
            "mind2",
            1,
            false,
            vec![],
            no_vote.clone(),
        ));
        bus.queue_response(mind_spoke(
            room_id,
            "mind3",
            1,
            false,
            vec![],
            no_vote.clone(),
        ));

        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            2,
            true,
            vec![],
            yes_vote.clone(),
        ));
        bus.queue_response(mind_spoke(
            room_id,
            "mind2",
            2,
            true,
            vec![],
            yes_vote.clone(),
        ));
        bus.queue_response(mind_spoke(
            room_id,
            "mind3",
            2,
            true,
            vec![],
            yes_vote.clone(),
        ));

        let coord = RoomCoordinator::new(bus, room_id, RoomKind::Conclave)
            .with_participants(vec!["mind1".into(), "mind2".into(), "mind3".into()])
            .with_max_rounds(3)
            .with_timeout(Duration::from_millis(100));

        let result = coord.run("test context").await;

        assert!(result.is_consensus());
        let decision = result.decision().unwrap();
        assert_eq!(decision.needs.len(), 1);
    }

    #[tokio::test]
    async fn test_timeout_returns_partial_decision() {
        let bus = MockBus::new();
        let room_id = "test-timeout";

        let proposal = make_proposal("p1", ProposalKind::Need, "approved thing");

        let mut votes = HashMap::new();
        votes.insert("p1".to_string(), Vote::Yes);

        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            1,
            true,
            vec![proposal],
            votes.clone(),
        ));
        bus.queue_response(mind_spoke(room_id, "mind2", 1, true, vec![], votes));

        let coord = RoomCoordinator::new(bus, room_id, RoomKind::Conclave)
            .with_participants(vec!["mind1".into(), "mind2".into(), "mind3".into()])
            .with_timeout(Duration::from_millis(10));

        let result = coord.run("test context").await;

        match result {
            RoomResult::Timeout {
                missing, decision, ..
            } => {
                assert_eq!(missing, vec!["mind3".to_string()]);
                assert_eq!(decision.needs.len(), 1);
            }
            _ => panic!("expected timeout"),
        }
    }

    #[tokio::test]
    async fn test_max_rounds_reached() {
        let bus = MockBus::new();
        let room_id = "test-max-rounds";

        for round in 1..=2 {
            bus.queue_response(mind_spoke(
                room_id,
                "mind1",
                round,
                false,
                vec![],
                HashMap::new(),
            ));
            bus.queue_response(mind_spoke(
                room_id,
                "mind2",
                round,
                false,
                vec![],
                HashMap::new(),
            ));
        }

        let coord = RoomCoordinator::new(bus, room_id, RoomKind::Conclave)
            .with_participants(vec!["mind1".into(), "mind2".into()])
            .with_max_rounds(2)
            .with_timeout(Duration::from_millis(100));

        let result = coord.run("test context").await;

        assert!(matches!(result, RoomResult::MaxRounds(_)));
    }

    #[tokio::test]
    async fn test_proposal_needs_threshold_votes() {
        let bus = MockBus::new();
        let room_id = "test-threshold";

        let proposal = make_proposal("p1", ProposalKind::Need, "needs 2/3 votes");

        let mut yes_vote = HashMap::new();
        yes_vote.insert("p1".to_string(), Vote::Yes);

        let mut no_vote = HashMap::new();
        no_vote.insert("p1".to_string(), Vote::No);

        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            1,
            true,
            vec![proposal],
            yes_vote,
        ));
        bus.queue_response(mind_spoke(
            room_id,
            "mind2",
            1,
            true,
            vec![],
            no_vote.clone(),
        ));
        bus.queue_response(mind_spoke(room_id, "mind3", 1, true, vec![], no_vote));

        let coord = RoomCoordinator::new(bus, room_id, RoomKind::Conclave)
            .with_participants(vec!["mind1".into(), "mind2".into(), "mind3".into()])
            .with_timeout(Duration::from_millis(100));

        let result = coord.run("test context").await;

        let decision = result.decision().unwrap();
        assert!(decision.needs.is_empty());
    }

    #[tokio::test]
    async fn test_emits_correct_events() {
        let bus = MockBus::new();
        let room_id = "test-events";
        let published = bus.published.clone();

        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            1,
            true,
            vec![],
            HashMap::new(),
        ));

        let coord = RoomCoordinator::new(bus, room_id, RoomKind::Conclave)
            .with_participants(vec!["mind1".into()])
            .with_timeout(Duration::from_millis(100));

        let _ = coord.run("test context").await;

        let events = published
            .lock()
            .unwrap()
            .iter()
            .filter_map(|m| {
                if let MessageData::Event { kind, .. } = &m.data {
                    Some(kind.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        assert!(events.contains(&"room_start".to_string()));
        assert!(events.contains(&"room_round".to_string()));
        assert!(events.contains(&"room_end".to_string()));
    }

    #[tokio::test]
    async fn test_want_proposal_tallied() {
        let bus = MockBus::new();
        let room_id = "test-want";

        let proposal = make_proposal("w1", ProposalKind::Want, "would like this");

        let mut votes = HashMap::new();
        votes.insert("w1".to_string(), Vote::Yes);

        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            1,
            true,
            vec![proposal],
            votes.clone(),
        ));
        bus.queue_response(mind_spoke(room_id, "mind2", 1, true, vec![], votes));

        let coord = RoomCoordinator::new(bus, room_id, RoomKind::Conclave)
            .with_participants(vec!["mind1".into(), "mind2".into()])
            .with_timeout(Duration::from_millis(100));

        let result = coord.run("test context").await;

        let decision = result.decision().unwrap();
        assert_eq!(decision.wants.len(), 1);
        assert_eq!(decision.wants[0].want, "would like this");
    }

    #[tokio::test]
    async fn test_ignores_wrong_room_responses() {
        let bus = MockBus::new();
        let room_id = "correct-room";

        bus.queue_response(mind_spoke(
            "wrong-room",
            "mind1",
            1,
            true,
            vec![],
            HashMap::new(),
        ));
        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            1,
            true,
            vec![],
            HashMap::new(),
        ));

        let coord = RoomCoordinator::new(bus, room_id, RoomKind::Conclave)
            .with_participants(vec!["mind1".into()])
            .with_timeout(Duration::from_millis(100));

        let result = coord.run("test context").await;

        assert!(result.is_consensus());
    }

    #[tokio::test]
    async fn test_ignores_wrong_round_responses() {
        let bus = MockBus::new();
        let room_id = "test-round";

        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            2,
            true,
            vec![],
            HashMap::new(),
        ));
        bus.queue_response(mind_spoke(
            room_id,
            "mind1",
            1,
            true,
            vec![],
            HashMap::new(),
        ));

        let coord = RoomCoordinator::new(bus, room_id, RoomKind::Conclave)
            .with_participants(vec!["mind1".into()])
            .with_timeout(Duration::from_millis(100));

        let result = coord.run("test context").await;

        assert!(result.is_consensus());
    }
}
