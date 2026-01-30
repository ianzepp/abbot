// Abbot - Persistent AI background bot framework.
//
// Abbot provides a multi-service architecture for running AI agents that
// can interact with the system through tools (bash, read, write, edit, etc).
// The architecture is inspired by human organization:
//
// - Head: AI decision maker that processes input and creates tasks
// - Heart: Background monitor that periodically summarizes state
// - Hand: Task executor that performs work using tools
// - Goal: Task coordinator that manages the lifecycle
//
// Services communicate via a message bus with SQLite persistence, enabling
// durability and recovery. Messages are routed through IRC-inspired scopes:
// - #channels for group chat
// - @mailboxes for direct messages
// - §task/<id> for isolated task threads

pub mod bus;
pub mod history;
pub mod llm;
pub mod runtime;
pub mod tools;

// Re-export only what the CLI needs
pub use bus::{Message, MessageOp, MessageData};
pub use history::Store;
