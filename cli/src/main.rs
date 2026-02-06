//! abbot - CLI for the Abbot daemon
//!
//! Unified CLI that handles both offline management (config, providers, memory,
//! plugins, service management) and RPC commands (audit, status, chat, etc.).
//! Offline commands work without a running daemon. RPC commands connect to the
//! daemon via a Unix domain socket.

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
#[command(name = "abbot")]
#[command(about = "CLI for the Abbot daemon")]
struct Cli {
    /// Path to config file (default: ~/.config/abbot/abbot.toml)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Path to the RPC unix socket (default: <workspace>/rpc.sock)
    #[arg(long)]
    sock: Option<PathBuf>,

    /// API server address (host:port) for monitor/tui commands
    #[arg(long)]
    addr: Option<String>,

    /// Output format (auto = pretty for TTY, JSON for pipes)
    #[arg(long, value_enum, default_value = "auto")]
    format: OutputFormat,

    /// Request timeout in seconds (for RPC commands)
    #[arg(long, default_value = "30")]
    timeout: u64,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    // === Offline commands (no daemon required) ===

    /// Show system configuration, status, and health
    Info,
    /// Manage abbot as a system service
    Service {
        #[command(subcommand)]
        action: commands::service::ServiceAction,
    },
    /// Reset workspace state (databases, memory, config)
    Reset {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
        /// Also delete and regenerate config file
        #[arg(long)]
        config: bool,
    },
    /// Manage model providers
    Providers {
        #[command(subcommand)]
        action: commands::providers::ProvidersAction,
    },
    /// Manage workspace plugins (tools)
    Plugin {
        #[command(subcommand)]
        action: commands::plugin::PluginAction,
    },
    /// Memory index management
    Memory {
        #[command(subcommand)]
        action: commands::memory::MemoryAction,
    },
    /// Query kernel frame logs
    Frames {
        #[command(subcommand)]
        action: commands::frames::FramesAction,
    },
    /// Stream frames from the daemon (websocket)
    Monitor {
        /// Filter by kind/name pattern (e.g., "chat:*", "need:*")
        #[arg(long)]
        filter: Option<String>,
    },
    /// Launch the TUI (assumes daemon is already running)
    Tui {
        /// Additional arguments to pass to abbot-tui
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    // === RPC commands (require running daemon) ===

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

async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();

    match cli.command {
        // Offline commands — no daemon connection needed
        Command::Info => {
            commands::info::run(cli.config).await
        }
        Command::Service { action } => {
            commands::service::run(action, cli.format)
        }
        Command::Reset { force, config: reset_config } => {
            commands::reset::run(cli.config, force, reset_config, cli.format)
        }
        Command::Providers { action } => {
            commands::providers::run(action, cli.format).await
        }
        Command::Plugin { action } => {
            commands::plugin::run(cli.config, action, cli.format)
        }
        Command::Memory { action } => {
            commands::memory::run(cli.config, action).await
        }
        Command::Frames { action } => {
            commands::frames::run(cli.config, action, cli.format).await
        }
        Command::Monitor { filter } => {
            commands::monitor::run(cli.config, cli.addr, filter).await
        }
        Command::Tui { args } => {
            commands::tui_cmd::run(cli.config, cli.addr, args)
        }

        // RPC commands — connect to daemon
        Command::Audit { action } => {
            let sock = resolve_sock(cli.sock)?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::audit::run(&mut client, action, timeout, cli.format).await
        }
        Command::Status { action } => {
            let sock = resolve_sock(cli.sock)?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::status::run(&mut client, action, timeout, cli.format).await
        }
        Command::Chat { action } => {
            let sock = resolve_sock(cli.sock)?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::chat::run(&mut client, action, timeout, cli.format).await
        }
        Command::Need { action } => {
            let sock = resolve_sock(cli.sock)?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::need::run(&mut client, action, timeout, cli.format).await
        }
        Command::Task { action } => {
            let sock = resolve_sock(cli.sock)?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::task::run(&mut client, action, timeout, cli.format).await
        }
        Command::Room { action } => {
            let sock = resolve_sock(cli.sock)?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::room::run(&mut client, action, timeout, cli.format).await
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
