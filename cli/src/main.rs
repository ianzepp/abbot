//! abbot-cli - CLI client for the Abbot daemon
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Entry point for the `abbot-cli` binary. Responsibilities:
//! 1. Parse command-line arguments via clap (global flags + subcommand tree).
//! 2. Resolve the RPC socket path (--sock > env > config file).
//! 3. Connect to the daemon and perform the handshake.
//! 4. Dispatch to the appropriate command module.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - The CLI is a thin transport layer: parse args, build JSON params, send one
//!   RPC call, format output. All business logic lives in the daemon.
//! - Socket resolution follows a precedence chain so scripts can override via
//!   flag or env, while interactive use falls through to the config file.
//! - Errors are printed to stderr with a non-zero exit code. No panics.

mod client;
mod commands;
mod config;
mod error;
mod frame;
mod output;

use std::path::PathBuf;
use std::process;
use std::time::Duration;

use clap::{Parser, Subcommand};

use client::RpcClient;
use error::CliError;
use output::OutputFormat;

// =============================================================================
// CLAP DEFINITIONS
// =============================================================================

#[derive(Parser)]
#[command(name = "abbot-cli")]
#[command(about = "CLI client for the Abbot daemon")]
struct Cli {
    /// Path to the RPC unix socket (default: <workspace>/rpc.sock)
    #[arg(long)]
    sock: Option<PathBuf>,

    /// Output format
    #[arg(long, value_enum, default_value = "json")]
    format: OutputFormat,

    /// Request timeout in seconds
    #[arg(long, default_value = "30")]
    timeout: u64,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Audit log operations
    Audit {
        #[command(subcommand)]
        action: commands::audit::AuditAction,
    },
    /// Runtime status
    Status {
        #[command(subcommand)]
        action: commands::status::StatusAction,
    },
    /// Chat operations
    Chat {
        #[command(subcommand)]
        action: commands::chat::ChatAction,
    },
    /// Need queue operations
    Need {
        #[command(subcommand)]
        action: commands::need::NeedAction,
    },
    /// Task queue operations
    Task {
        #[command(subcommand)]
        action: commands::task::TaskAction,
    },
    /// Room operations
    Room {
        #[command(subcommand)]
        action: commands::room::RoomAction,
    },
}

// =============================================================================
// SOCKET RESOLUTION
// =============================================================================

/// Resolve the RPC socket path from the available sources.
///
/// WHY precedence chain: Scripts need deterministic paths (--sock or env),
/// while interactive use should "just work" from the config file. The chain
/// mirrors how other CLI tools (kubectl, docker) resolve endpoints.
///
/// Resolution order: --sock flag > ABBOT_RPC_SOCK env > config file
fn resolve_sock(cli_sock: Option<PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(path) = cli_sock {
        return Ok(path);
    }
    if let Ok(path) = std::env::var("ABBOT_RPC_SOCK") {
        return Ok(PathBuf::from(path));
    }
    config::default_rpc_sock().ok_or_else(|| {
        CliError::Config(
            "could not determine rpc.sock path; set workspace in ~/.config/abbot/abbot.toml or pass --sock".into(),
        )
    })
}

// =============================================================================
// ENTRY POINT
// =============================================================================

/// Parse args, connect, dispatch.
async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    let sock = resolve_sock(cli.sock)?;
    let timeout = Duration::from_secs(cli.timeout);
    let format = cli.format;

    let mut client = RpcClient::connect(&sock).await?;

    match cli.command {
        Command::Audit { action } => {
            commands::audit::run(&mut client, action, timeout, format).await
        }
        Command::Status { action } => {
            commands::status::run(&mut client, action, timeout, format).await
        }
        Command::Chat { action } => {
            commands::chat::run(&mut client, action, timeout, format).await
        }
        Command::Need { action } => {
            commands::need::run(&mut client, action, timeout, format).await
        }
        Command::Task { action } => {
            commands::task::run(&mut client, action, timeout, format).await
        }
        Command::Room { action } => {
            commands::room::run(&mut client, action, timeout, format).await
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        process::exit(1);
    }
}
