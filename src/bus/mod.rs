// Message bus system for internal communication between services.
//
// The bus provides a pub/sub architecture with IRC-inspired scoping:
// - #channels for group chat (e.g., #general)
// - @mailboxes for direct messages (e.g., @abbot)
// - §task/<id> for task-specific communication
//
// Messages are persisted to SQLite via the RuntimeBus wrapper, ensuring
// durability and allowing services to recover state after restarts.

mod channel;
mod hub;
mod message;
mod scope;

pub use channel::Channel;
pub use hub::Hub;
pub use message::Origin;
pub use message::{
    Message, MessageData, MessageOp, NeedMsg, NeedPriority, Stats, TaskMsg, WantMsg, respond,
};
pub use scope::Scope;
