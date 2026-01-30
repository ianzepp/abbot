// Abbot - Persistent AI background daemon.
//
// Abbot provides a multi-service architecture for running AI agents that
// can interact with the system through tools (bash, read, write, edit, etc).
// The architecture is inspired by distributed cognition:
//
// - Head: AI decision maker that processes input and creates tasks
// - Heart: Background monitor that periodically summarizes state
// - Hand: Task executor that performs work using tools
// - Goal: Task coordinator that manages the lifecycle
//
// Services communicate via a message bus with SQLite persistence, enabling
// durability and recovery. Scopes route messages:
// - main: shared world scope
// - head/<id>/mail: private inbox for a head
// - head/<id>/stm, head/<id>/ltm: memory scopes
// - task/<id>: isolated task threads
//
// Users interact via OpenAI-compatible API at /v1/chat/completions.

pub mod bus;
pub mod history;
pub mod llm;
pub mod runtime;
pub mod server;
pub mod tools;

pub use bus::{Message, MessageData, MessageOp};
pub use history::Store;
