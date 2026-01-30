pub mod bus;
pub mod api;
pub mod history;
pub mod llm;
pub mod runtime;
pub mod tools;
pub mod irc;
pub mod socket;

// Re-export only what the CLI needs
pub use bus::{Message, MessageOp, MessageData};
pub use history::Store;
