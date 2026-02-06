// SQLite persistence for durable runtime state.
//
// Conversation history is stored in logs.db via the kernel audit log.

pub mod store;

pub use store::{Store, ToolRegistrySummary, ToolRegistryTool};
