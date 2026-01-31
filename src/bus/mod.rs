// Message bus system for internal communication between services.
//
// The bus provides a pub/sub architecture with IRC-inspired scoping:
// - #channels for group chat (e.g., #general)
// - @mailboxes for direct messages (e.g., @abbot)
// - §task/<id> for task-specific communication
//
// Messages are persisted to SQLite via the RuntimeBus wrapper, ensuring
// durability and allowing services to recover state after restarts.

mod message;
mod channel;
mod hub;
mod scope;

pub use message::{Message, MessageOp, MessageData, TaskMsg, NeedMsg, NeedPriority, Stats, respond};
pub use message::Origin;
pub use channel::Channel;
pub use hub::Hub;
pub use scope::Scope;
