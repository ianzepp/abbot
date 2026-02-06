// Abbot - Persistent AI background daemon.
//
// Abbot provides a multi-service architecture for running AI agents that
// can interact with the system through tools (bash, read, write, edit, etc).
// The architecture is inspired by distributed cognition:
//
// - Head: AI decision maker that processes input and creates tasks
// - Mind: Background reflector that maintains long-term memory
// - Hand: Task executor that performs work using tools
// - Task: Coordinator that manages the task lifecycle
//
// Services communicate via kernel syscalls/streams. Persistence uses store.db
// for durable state and logs.db for conversation history.
//
// Users interact via OpenAI-compatible API at /v1/chat/completions.

#[macro_export]
macro_rules! tool_spec {
    ($name:literal) => {
        $crate::llm::ToolSpec::from_json_str(include_str!(concat!($name, ".json")))
    };
}

#[macro_export]
macro_rules! tool_specs {
    ($($name:literal),* $(,)?) => {
        vec![$($crate::tool_spec!($name)),*]
    };
}

pub mod agent_tools;
pub mod tools;
pub mod ems;
pub mod hal;
pub mod history;
pub mod kernel;
pub mod llm;
pub mod recall;
pub mod runtime;
pub mod scope;
pub mod server;
pub mod syscalls;
pub mod vfs;

pub use history::Store;
pub use scope::Scope;
