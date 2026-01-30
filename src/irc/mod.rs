// IRC server for human interaction with the bot.
//
// Provides a minimal IRC server that bridges IRC clients to the bus.
// Humans can join channels, send messages, and receive responses through
// their preferred IRC client. Messages are formatted appropriately for
// IRC's line-oriented protocol.

mod protocol;
mod server;
mod connection;
mod format;

pub use server::Server;
pub use format::{markdown_to_irc, tool_notice};
