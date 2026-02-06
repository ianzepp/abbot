//! Command modules - Clap subcommand definitions and handlers

// RPC-based commands (require running daemon)
pub mod audit;
pub mod chat;
pub mod need;
pub mod room;
pub mod status;
pub mod task;

// Offline commands (moved from daemon binary)
pub mod frames;
pub mod info;
pub mod memory;
pub mod monitor;
pub mod plugin;
pub mod providers;
pub mod reset;
pub mod service;
pub mod tui_cmd;
