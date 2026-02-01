// Room: a deliberation space where multiple Minds reach consensus.
//
// Rooms are blocking - participants iterate until they agree or timeout.
// The primary room is the Conclave where MindManager, HeadManager, and HandManager deliberate
// on strategic decisions (needs, wants, LTM updates).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Room {
    pub id: String,
    pub kind: RoomKind,
    pub minds: Vec<MindPersona>,
    pub transcript: Vec<RoomMessage>,
    pub status: RoomStatus,
    pub decision: Option<RoomDecision>,
    pub max_rounds: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomKind {
    Conclave,
    Autonomy,
}

#[derive(Debug, Clone)]
pub struct MindPersona {
    pub name: String,
    pub role: String,
    pub model: String,
    pub temperature: f32,
    pub system_prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMessage {
    pub mind: String,
    pub content: String,
    pub round: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomStatus {
    Open,
    Closed,
    Timeout,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoomDecision {
    pub needs: Vec<NeedProposal>,
    pub wants: Vec<WantProposal>,
    pub ltm_ops: Vec<LtmProposal>,
    pub self_ops: Vec<SelfProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedProposal {
    pub need: String,
    pub context: String,
    pub priority: String,
    #[serde(default)]
    pub reconvene: bool, // trigger conclave when fulfilled
    pub votes: Vec<String>, // which minds agreed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WantProposal {
    pub want: String,
    pub context: String,
    pub priority: String,
    pub proposer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LtmProposal {
    pub kind: String, // append, replace, remove
    pub content: String,
    pub pattern: String,
    pub proposer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfProposal {
    pub kind: String, // append, replace, remove
    pub content: String,
    pub pattern: String,
    pub proposer: String,
}

impl Room {
    pub fn conclave(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: RoomKind::Conclave,
            minds: vec![
                MindPersona::mind_manager(),
                MindPersona::head_manager(),
                MindPersona::hand_manager(),
            ],
            transcript: Vec::new(),
            status: RoomStatus::Open,
            decision: None,
            max_rounds: 5,
        }
    }

    pub fn autonomy(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: RoomKind::Autonomy,
            minds: vec![
                MindPersona::mind_manager(),
                MindPersona::head_manager(),
                MindPersona::hand_manager(),
            ],
            transcript: Vec::new(),
            status: RoomStatus::Open,
            decision: None,
            max_rounds: 3,
        }
    }

    pub fn add_message(
        &mut self,
        mind: impl Into<String>,
        content: impl Into<String>,
        round: usize,
    ) {
        self.transcript.push(RoomMessage {
            mind: mind.into(),
            content: content.into(),
            round,
        });
    }

    pub fn close(&mut self, decision: RoomDecision) {
        self.decision = Some(decision);
        self.status = RoomStatus::Closed;
    }

    pub fn timeout(&mut self) {
        self.status = RoomStatus::Timeout;
    }
}

impl MindPersona {
    pub fn mind_manager() -> Self {
        Self {
            name: "MindManager".to_string(),
            role: "Strategic direction".to_string(),
            model: "haiku".to_string(),
            temperature: 0.8,
            system_prompt: include_str!("mind_manager.md").to_string(),
        }
    }

    pub fn head_manager() -> Self {
        Self {
            name: "HeadManager".to_string(),
            role: "Tactical decisions".to_string(),
            model: "haiku".to_string(),
            temperature: 0.5,
            system_prompt: include_str!("head_manager.md").to_string(),
        }
    }

    pub fn hand_manager() -> Self {
        Self {
            name: "HandManager".to_string(),
            role: "Operational execution".to_string(),
            model: "haiku".to_string(),
            temperature: 0.3,
            system_prompt: include_str!("hand_manager.md").to_string(),
        }
    }
}
