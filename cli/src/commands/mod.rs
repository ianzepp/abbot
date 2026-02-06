//! Command modules - Clap subcommand definitions and RPC dispatch
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Each module defines a clap `Subcommand` enum for its subcommands and a
//! `run()` function that builds the JSON params, calls `client.call()`, and
//! delegates to `output::print_response()`. No business logic lives here;
//! these are pure transport adapters.

pub mod audit;
pub mod chat;
pub mod need;
pub mod room;
pub mod status;
pub mod task;
