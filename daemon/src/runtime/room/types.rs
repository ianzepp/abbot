//! Room Types - Domain model for multi-agent deliberation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Defines the core data structures for the room system: rooms, participants,
//! messages, decisions, and proposal types. A Room is a bounded deliberation
//! space where Participants iterate through rounds until consensus or timeout.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Room types determine behavior: Conclave (strategic), Autonomy (tactical),
//!   Work (code changes in isolated worktree). All share the same Room struct
//!   but differ in participant count, round limits, and tool availability.
//! - Proposals are typed: need, want, ltm, self, control. Each maps to a
//!   specific side effect when approved (enqueue need, update memory, etc.).
//! - Backward compatibility: Type aliases `MindPersona` and `RoomKind` are
//!   preserved for callers that haven't migrated to the new names.
//!
//! TRADE-OFFS
//! ==========
//! - Participant system prompts are loaded via `include_str!` at compile time.
//!   This couples prompt content to binary releases but eliminates runtime
//!   file-not-found errors and simplifies deployment.
//! - All room types use the same three participants (MindManager, HeadManager,
//!   HandManager). Future room types may need different participant sets.

use serde::{Deserialize, Serialize};

// =============================================================================
// ROOM
// =============================================================================
//
// WHY a single Room struct for all types: Conclave, Autonomy, and Work rooms
// share the same lifecycle (open -> rounds -> close/timeout) and differ only
// in configuration (max_rounds, tool specs, grammar). A unified struct avoids
// duplicating the deliberation loop for each room type.

/// Bounded deliberation space where participants iterate to consensus.
///
/// WHY this exists: Rooms isolate multi-agent decision-making from the main
/// chat turn flow, preventing interference and enabling parallel deliberation.
///
/// INVARIANTS:
/// ----------
/// INV-1: status transitions are one-way: Open -> Closed | Timeout
/// INV-2: decision is Some only when status is Closed
#[derive(Debug, Clone)]
pub struct Room {
    pub id: String,
    /// WHY room_type not kind: Merges the former RoomKind (kernel) and
    /// RoomType (bundle) into a single enum with all three variants.
    pub room_type: RoomType,
    pub participants: Vec<Participant>,
    pub transcript: Vec<RoomMessage>,
    pub status: RoomStatus,
    pub decision: Option<RoomDecision>,
    /// WHY configurable: Conclave (5), Autonomy (3), Work (10) have different
    /// iteration budgets reflecting their complexity.
    pub max_rounds: usize,
}

// =============================================================================
// ROOM TYPE
// =============================================================================
//
// WHY three types: Conclave handles strategic planning (needs, wants, memory),
// Autonomy handles post-idle reflection (lighter, fewer rounds), and Work
// handles code changes in isolated git worktrees.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomType {
    /// Strategic planning: creates needs, wants, updates LTM/identity.
    Conclave,
    /// Post-idle reflection: lighter than conclave, fewer rounds.
    Autonomy,
    /// Code execution: tools operate in an isolated git worktree.
    Work,
}

impl RoomType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "conclave" => Some(Self::Conclave),
            "autonomy" => Some(Self::Autonomy),
            "work" => Some(Self::Work),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Conclave => "conclave",
            Self::Autonomy => "autonomy",
            Self::Work => "work",
        }
    }
}

// =============================================================================
// PARTICIPANT
// =============================================================================
//
// WHY three fixed participants: The MindManager/HeadManager/HandManager triad
// mirrors the head/hand/mind agent architecture, ensuring each perspective
// (strategic, tactical, operational) is represented in deliberation.

/// AI persona that participates in room deliberation.
///
/// WHY this exists: Each participant has a distinct system prompt, role, and
/// temperature setting that shapes their contribution to the deliberation.
#[derive(Debug, Clone)]
pub struct Participant {
    pub name: String,
    pub role: String,
    pub model: String,
    /// WHY different temperatures: MindManager (0.8) explores creative options,
    /// HeadManager (0.5) balances exploration/precision, HandManager (0.3)
    /// favors concrete, deterministic responses.
    pub temperature: f32,
    pub system_prompt: String,
}

// =============================================================================
// TRANSCRIPT AND STATUS
// =============================================================================

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

// =============================================================================
// DECISION AND PROPOSALS
// =============================================================================
//
// WHY typed proposals: Each proposal type maps to a distinct side effect in
// execute_decision(). Typing them at the data level enables the ProposalTracker
// to validate, tally votes, and dispatch correctly without string matching.

/// Aggregate decision produced by room deliberation.
///
/// WHY this exists: Collects all approved proposals into a single structure
/// that execute_decision() can process atomically.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoomDecision {
    pub needs: Vec<NeedProposal>,
    pub wants: Vec<WantProposal>,
    pub ltm_ops: Vec<LtmProposal>,
    pub self_ops: Vec<SelfProposal>,
    #[serde(default)]
    pub control_ops: Vec<ControlProposal>,
}

/// Proposal to create a strategic need for a head to address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedProposal {
    pub need: String,
    pub context: String,
    pub priority: String,
    /// WHY reconvene: Urgent needs trigger an immediate follow-up conclave
    /// to assess the result, rather than waiting for the next idle cycle.
    #[serde(default)]
    pub reconvene: bool,
    pub votes: Vec<String>,
}

/// Proposal to add an aspirational item to the wants pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WantProposal {
    pub want: String,
    pub context: String,
    pub priority: String,
    pub proposer: String,
}

/// Proposal to modify long-term memory (append/replace/remove).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LtmProposal {
    /// Operation kind: "append", "replace", or "remove".
    pub kind: String,
    pub content: String,
    /// WHY pattern: For replace/remove, identifies the text to find in the
    /// existing LTM before applying the operation.
    pub pattern: String,
    pub proposer: String,
}

/// Proposal to modify the collective self-identity (append/replace/remove).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfProposal {
    pub kind: String,
    pub content: String,
    pub pattern: String,
    pub proposer: String,
}

/// Proposal for system-level control actions (e.g., reboot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlProposal {
    /// Control action kind (currently only "reboot_collective").
    pub kind: String,
    /// WHY mode: "hard" clears all queues immediately, "soft" waits for
    /// current work to complete before rebooting.
    pub mode: String,
    pub reason: String,
    pub proposer: String,
}

// =============================================================================
// ROOM FACTORY METHODS
// =============================================================================
//
// WHY factory methods: Each room type has distinct defaults (max_rounds,
// participants). Factory methods encode these defaults so callers don't need
// to know the configuration details.

impl Room {
    /// Create a conclave room (strategic planning, 5 rounds).
    pub fn conclave(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            room_type: RoomType::Conclave,
            participants: vec![
                Participant::mind_manager(),
                Participant::head_manager(),
                Participant::hand_manager(),
            ],
            transcript: Vec::new(),
            status: RoomStatus::Open,
            decision: None,
            max_rounds: 5,
        }
    }

    /// Create an autonomy room (post-idle reflection, 3 rounds).
    pub fn autonomy(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            room_type: RoomType::Autonomy,
            participants: vec![
                Participant::mind_manager(),
                Participant::head_manager(),
                Participant::hand_manager(),
            ],
            transcript: Vec::new(),
            status: RoomStatus::Open,
            decision: None,
            max_rounds: 3,
        }
    }

    /// Create a work room (code execution in isolated worktree, 10 rounds).
    pub fn work(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            room_type: RoomType::Work,
            participants: vec![
                Participant::mind_manager(),
                Participant::head_manager(),
                Participant::hand_manager(),
            ],
            transcript: Vec::new(),
            status: RoomStatus::Open,
            decision: None,
            max_rounds: 10,
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

// =============================================================================
// PARTICIPANT CONSTRUCTORS
// =============================================================================

impl Participant {
    pub fn mind_manager() -> Self {
        Self {
            name: "MindManager".to_string(),
            role: "Strategic direction".to_string(),
            model: "haiku".to_string(),
            temperature: 0.8,
            system_prompt: include_str!("../mind_manager.md").to_string(),
        }
    }

    pub fn head_manager() -> Self {
        Self {
            name: "HeadManager".to_string(),
            role: "Tactical decisions".to_string(),
            model: "haiku".to_string(),
            temperature: 0.5,
            system_prompt: include_str!("../head_manager.md").to_string(),
        }
    }

    pub fn hand_manager() -> Self {
        Self {
            name: "HandManager".to_string(),
            role: "Operational execution".to_string(),
            model: "haiku".to_string(),
            temperature: 0.3,
            system_prompt: include_str!("../hand_manager.md").to_string(),
        }
    }
}

// =============================================================================
// BACKWARD COMPATIBILITY ALIASES
// =============================================================================
//
// WHY aliases: External code (syscalls, coordinator) may still reference the
// old names. These aliases prevent breakage during migration without requiring
// a coordinated rename across all call sites.

pub type MindPersona = Participant;
pub type RoomKind = RoomType;
