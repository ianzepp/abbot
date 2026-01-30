// SQLite persistence for messages and state recovery.
//
// All messages published to the bus are stored in SQLite, enabling services
// to recover their state after restarts by replaying recent messages. The
// store also supports querying history for TUI display and tail operations.

pub mod store;

pub use store::Store;
