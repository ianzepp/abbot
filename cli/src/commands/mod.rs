//! Command modules - Clap subcommand definitions and handlers

// Offline commands (no daemon required)
pub mod config_cmd;
pub mod doctor;
pub mod frames;
pub mod info;
pub mod init;
pub mod monitor;
pub mod mounts;
pub mod providers;
pub mod reset;
pub mod restart;
pub mod run_cmd;
pub mod service;
pub mod start;
pub mod stop;
pub mod tui_cmd;
pub mod use_cmd;

// RPC-based commands (require running daemon)
pub mod chat;
pub mod ems;

// Diagnostic scripts (offline, no daemon required)
pub mod scripts;

// Hybrid commands (offline + optional RPC)
pub mod status;
