//! Room Module - Multi-agent deliberation and work execution
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The room module implements isolated execution contexts where multiple AI
//! participants collaborate to reach decisions or complete work. Rooms replace
//! the former Conclave/MindService architecture with a unified model that
//! supports three room types: strategic conclaves, tactical autonomy sessions,
//! and isolated work rooms with git worktrees.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Unified execution model: All room types share the same runner loop,
//!   differing only in prompt grammar, tool sets, and round limits.
//! - Coordinator-driven scheduling: The coordinator polls kernel idle state
//!   and dispatches rooms via `room:create` + `room:run` syscalls.
//! - Tool-based interaction: Participants use structured tools (propose, vote,
//!   done) rather than free-form JSON, enabling validation and tracking.
//!
//! MODULE STRUCTURE
//! ================
//! - types: Room, Participant, RoomDecision, proposal types
//! - config: RoomConfig loaded from AppConfig + workspace TOML
//! - bundle: Context building (system prompt, workspace state, activity)
//! - coordinator: Idle-driven scheduling loop (replaces MindService)
//! - tools: Tool specs and ProposalTracker for deliberation
//! - runner: Core multi-round execution loop (replaces Conclave)
//! - worktree: Git worktree provisioning for work rooms

mod types;
mod config;
pub(crate) mod bundle;
mod coordinator;
mod tools;
mod runner;
mod worktree;

pub use types::{
    ControlProposal, LtmProposal, MindPersona, NeedProposal, Participant, Room, RoomDecision,
    RoomKind, RoomMessage, RoomStatus, RoomType, SelfProposal, WantProposal,
};
pub use config::RoomConfig;
pub use bundle::{FeverMode, RoomBundleBuilder, RoomBundleConfig, WakeMode};
pub use coordinator::RoomCoordinator;
pub use runner::RoomRunner;
