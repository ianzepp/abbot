//! Room Module - Parallel multi-agent execution
//!
//! The room module implements isolated execution contexts where multiple AI
//! agents collaborate in parallel rounds. Each agent runs independently with
//! a private conversation history, shares a common transcript, and synchronizes
//! at round boundaries.
//!
//! MODULE STRUCTURE
//! ================
//! - types: Room, RoomAgent, RoomType, AgentRoundResult, TranscriptEntry
//! - config: RoomConfig loaded from AppConfig + workspace TOML
//! - bundle: Context building (system prompt, workspace state, activity)
//! - coordinator: Idle-driven scheduling loop
//! - tools: Tool catalogs for room agents
//! - runner: Core parallel round execution loop
//! - worktree: Git worktree provisioning for work rooms

mod types;
mod config;
pub(crate) mod bundle;
mod coordinator;
mod tools;
mod runner;
mod worktree;

pub use types::{
    Room, RoomAgent, RoomType, RoomKind, AgentRoundResult, TranscriptEntry,
};
pub use config::RoomConfig;
pub use bundle::{FeverMode, RoomBundleBuilder, RoomBundleConfig, WakeMode};
pub use coordinator::RoomCoordinator;
pub use runner::RoomRunner;
