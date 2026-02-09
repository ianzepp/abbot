//! Head module - Bundle building, config, and tool catalog for head agents.
//!
//! The head agent is the primary interactive agent that responds to user
//! chat messages. This module provides:
//! - `HeadBundleBuilder` / `HeadBundleConfig` - System prompt and context assembly
//! - `HeadConfig` - LLM and pool configuration
//! - Config helpers (`head_context_budget_tokens`, `head_time_gap_marker_minutes`)
//! - `head_room_catalog()` - Tool catalog for head agents in rooms (in syscalls/dispatch.rs)
//!
//! Room creation and message injection are handled by `chat:message` syscall
//! via the RoomRegistry. Head agents run inside rooms as RoomAgents.

mod bundle;
pub(crate) mod config;

// Re-exports for runtime/mod.rs
pub use bundle::{HeadBundleBuilder, HeadBundleConfig};
pub use config::HeadConfig;
