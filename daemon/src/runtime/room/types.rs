//! Room Types - Domain model for parallel multi-agent execution
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Defines the core data structures for the room system. A Room is a bounded
//! execution space where N agents run independently with private conversation
//! histories, share a common transcript, and synchronize at round boundaries.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Agents are isolated**: Each agent has private `messages` (its LLM conversation
//!   history). Cross-agent communication happens only through the shared `transcript`.
//! - **Round-based synchronization**: Agents run in parallel within a round, then
//!   synchronize. This prevents race conditions while allowing concurrent execution.
//! - **Signal-based termination**: Agents opt out via `noop_done` (permanent) or
//!   `noop_signal` (done for this round). The room ends when all agents are inactive
//!   or quiescence is reached (nobody spoke).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::hal::llm::{ChatMessage, ToolSpec};

use super::door::Door;

// =============================================================================
// ROOM
// =============================================================================

/// Bounded execution space where agents run in parallel per round.
///
/// WHY: Provides a structured container for multi-agent collaboration with
/// explicit round limits and transcript tracking. The `worktree` flag enables
/// optional filesystem isolation for code-execution rooms.
#[derive(Debug, Clone)]
pub struct Room {
    pub id: String,
    /// Human-readable name (e.g., "issue-42"). Used for scope naming: "room/<name>".
    pub name: String,
    pub room_type: RoomType,
    /// Purpose prompt describing why this room was convened.
    pub prompt: String,
    pub agents: Vec<RoomAgent>,
    /// Shared transcript visible to all agents (injected at round boundaries).
    pub transcript: Vec<TranscriptEntry>,
    pub max_rounds: usize,
    /// Whether to provision an isolated git worktree for this room.
    pub worktree: bool,
    /// Optional bidirectional bridge to an external channel (e.g., TUI, web UI).
    /// When present, agent output is streamed to the client via chat:* syscalls.
    pub door: Option<Arc<dyn Door>>,
}

// =============================================================================
// ROOM TYPE
// =============================================================================

/// Classification of room execution mode.
///
/// WHY: Different room types have different default behaviors (e.g., Work rooms
/// get git worktree isolation). This enum drives those defaults without requiring
/// callers to specify every configuration detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomType {
    /// General-purpose room for multi-agent collaboration.
    General,
    /// Code execution: tools operate in an isolated git worktree.
    Work,
}

impl RoomType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "general" => Some(Self::General),
            "work" => Some(Self::Work),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Work => "work",
        }
    }
}

// =============================================================================
// ROOM AGENT
// =============================================================================

/// An agent participating in a room with private conversation history.
///
/// WHY: Each agent needs its own LLM conversation state (system prompt, message
/// history, available tools) while sharing a room-level transcript. The `active`
/// flag tracks whether the agent has permanently left via `noop_done`.
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
// ROUND RESULT
// =============================================================================

/// Outcome of a single agent's inner tool loop within one round.
///
/// WHY: The runner needs to distinguish between three exit conditions to decide
/// whether the agent stays active, and whether the room should continue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRoundResult {
    /// Agent produced visible text (participated in discussion).
    Spoke,
    /// Agent called noop_signal (done for this round, ready to listen next round).
    Signal,
    /// Agent called noop_done (permanently leaving the room).
    Done,
}

// =============================================================================
// TRANSCRIPT
// =============================================================================

/// A single entry in the shared room transcript.
///
/// WHY: The transcript is the only cross-agent communication channel. Entries
/// are tagged with agent name and round number so agents can distinguish who
/// said what and when.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub agent: String,
    pub content: String,
    pub round: usize,
}

// =============================================================================
// CONSTRUCTORS
// =============================================================================

impl Room {
    /// Create a new room with the given agents.
    ///
    /// WHY `worktree` defaults from `room_type`: Work rooms need filesystem isolation
    /// by convention. Callers can override this after construction if needed.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        room_type: RoomType,
        prompt: impl Into<String>,
        agents: Vec<RoomAgent>,
        max_rounds: usize,
    ) -> Self {
        let worktree = room_type == RoomType::Work;
        Self {
            id: id.into(),
            name: name.into(),
            room_type,
            prompt: prompt.into(),
            agents,
            transcript: Vec::new(),
            max_rounds,
            worktree,
            door: None,
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
