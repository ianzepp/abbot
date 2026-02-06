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

pub(crate) mod bundle;
mod config;
mod coordinator;
mod runner;
mod tools;
mod types;
mod worktree;

pub use bundle::{RoomBundleBuilder, RoomBundleConfig, WakeMode};
pub use config::RoomConfig;
pub use coordinator::RoomCoordinator;
pub use runner::RoomRunner;
pub use types::{AgentRoundResult, Room, RoomAgent, RoomKind, RoomType, TranscriptEntry};
