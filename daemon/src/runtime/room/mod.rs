//! Room Module - Parallel multi-agent execution
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The room module implements isolated execution contexts where multiple AI
//! agents collaborate in parallel rounds. Each agent runs independently with
//! a private conversation history, shares a common transcript, and synchronizes
//! at round boundaries.
//!
//! MODULE STRUCTURE
//! ================
//! - `types`: Room, RoomAgent, RoomType, AgentRoundResult, TranscriptEntry
//! - `config`: RoomConfig loaded from AppConfig + workspace TOML
//! - `tools`: Shared workspace context builder
//! - `runner`: Core parallel round execution loop (tokio::JoinSet)
//! - `worktree`: Git worktree provisioning for Work rooms

mod config;
pub mod door;
mod runner;
pub(crate) mod tools;
mod types;
mod worktree;

pub use config::RoomConfig;
pub use door::Door;
pub use runner::RoomRunner;
pub use types::{AgentRoundResult, Room, RoomAgent, RoomType, TranscriptEntry};
