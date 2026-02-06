//! Room Types - Domain model for parallel multi-agent execution
//!
//! Defines the core data structures for the room system: rooms, agents,
//! transcript entries, and round results. A Room is a bounded execution
//! space where N agents run independently with private conversation histories,
//! share a common transcript, and synchronize at round boundaries.

use serde::{Deserialize, Serialize};

use crate::hal::llm::{ChatMessage, ToolSpec};

// =============================================================================
// ROOM
// =============================================================================

/// Bounded execution space where agents run in parallel per round.
#[derive(Debug, Clone)]
pub struct Room {
    pub id: String,
    pub room_type: RoomType,
    /// Purpose prompt describing why this room was convened.
    pub prompt: String,
    pub agents: Vec<RoomAgent>,
    pub transcript: Vec<TranscriptEntry>,
    pub max_rounds: usize,
}

// =============================================================================
// ROOM TYPE
// =============================================================================

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
    #[allow(clippy::should_implement_trait)]
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
// ROOM AGENT
// =============================================================================

/// An agent participating in a room with private conversation history.
#[derive(Debug, Clone)]
pub struct RoomAgent {
    pub name: String,
    pub role: String,
    pub system_prompt: String,
    pub tools: Vec<ToolSpec>,
    /// Private conversation history (system + user/assistant/tool messages).
    pub messages: Vec<ChatMessage>,
    /// False after agent calls noop_done (permanently left the room).
    pub active: bool,
}

// =============================================================================
// AGENT ROUND RESULT
// =============================================================================

/// Outcome of a single agent's inner tool loop within one round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRoundResult {
    /// Agent produced visible text (participated in discussion).
    Spoke,
    /// Agent called noop_signal (done for this round, ready to listen).
    Signal,
    /// Agent called noop_done (permanently leaving the room).
    Done,
}

// =============================================================================
// TRANSCRIPT ENTRY
// =============================================================================

/// A single entry in the shared room transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub agent: String,
    pub content: String,
    pub round: usize,
}

// =============================================================================
// ROOM FACTORY METHODS
// =============================================================================

impl Room {
    /// Create a new room with the given agents.
    pub fn new(
        id: impl Into<String>,
        room_type: RoomType,
        prompt: impl Into<String>,
        agents: Vec<RoomAgent>,
        max_rounds: usize,
    ) -> Self {
        Self {
            id: id.into(),
            room_type,
            prompt: prompt.into(),
            agents,
            transcript: Vec::new(),
            max_rounds,
        }
    }
}

impl RoomAgent {
    pub fn new(
        name: impl Into<String>,
        role: impl Into<String>,
        system_prompt: impl Into<String>,
        tools: Vec<ToolSpec>,
    ) -> Self {
        Self {
            name: name.into(),
            role: role.into(),
            system_prompt: system_prompt.into(),
            tools,
            messages: Vec::new(),
            active: true,
        }
    }
}

// =============================================================================
// BACKWARD COMPATIBILITY ALIASES
// =============================================================================

pub type RoomKind = RoomType;
