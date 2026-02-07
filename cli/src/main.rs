//! abbot - CLI for the Abbot daemon
//!
//! Unified CLI that handles both offline management (config, providers,
//! service management) and RPC commands (status, chat, ems, etc.).
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
    /// Path to config file (default: ~/.abbot/abbot.toml)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Path to the RPC unix socket (default: ~/.abbot/rpc.sock)
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
    // === SETUP ===
    /// Initialize Abbot configuration (first-time setup)
    Init {
        /// Delete ~/.abbot/ entirely before re-initializing
        #[arg(long)]
        clean: bool,
    },
    /// Read or write configuration (~/.abbot/abbot.toml)
    Config {
        #[command(subcommand)]
        action: commands::config_cmd::ConfigAction,
    },
    /// Manage model providers
    Providers {
        #[command(subcommand)]
        action: commands::providers::ProvidersAction,
    },
    /// Switch the active LLM provider/model and run preflight
    Use {
        /// Provider name (e.g., anthropic, openai, openrouter, ollama)
        provider: String,
        /// Model name (e.g., claude-sonnet-4-20250514, gpt-4.1)
        model: String,
    },
    /// Reset workspace state (databases, memory)
    Reset {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },

    // === LIFECYCLE ===
    /// Start the daemon
    Start,
    /// Stop the daemon
    Stop,
    /// Restart the daemon (stop + start)
    Restart,
    /// Show daemon status (service + runtime)
    Status,

    // === DIAGNOSTICS ===
    /// Check system health (offline preflight checks)
    Doctor,
    /// Show system configuration and environment
    Info,

    // === DATA ===
    /// Query frame history (structured, from SQLite)
    Frames {
        #[command(subcommand)]
        action: commands::frames::FramesAction,
    },
    /// Stream live frames (WebSocket)
    Monitor {
        /// Filter by kind/name pattern (e.g., "chat:*", "need:*")
        #[arg(long)]
        filter: Option<String>,
    },

    // === INTERACTION ===
    /// Chat operations
    Chat {
        #[command(subcommand)]
        action: commands::chat::ChatAction,
    },
    /// Launch the TUI (assumes daemon is already running)
    Tui {
        /// Additional arguments to pass to abbot-tui
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    // === SERVICE ===
    /// Manage system service (install/uninstall)
    Service {
        #[command(subcommand)]
        action: commands::service::ServiceAction,
    },

    // === EMS ===
    /// Entity management (needs, tasks, rooms)
    Ems {
        #[command(subcommand)]
        action: commands::ems::EmsAction,
    },
}

// =============================================================================
// SOCKET RESOLUTION
// =============================================================================

pub fn resolve_sock(
    cli_sock: Option<PathBuf>,
    cli_config: Option<&std::path::Path>,
) -> Result<PathBuf, CliError> {
    if let Some(path) = cli_sock {
        return Ok(path);
    }
    if let Ok(path) = std::env::var("ABBOT_RPC_SOCK") {
        return Ok(PathBuf::from(path));
    }
    config::default_rpc_sock(cli_config).ok_or_else(|| {
        CliError::Config(
            "could not determine rpc.sock path; pass --sock or ensure ~/.abbot/ exists".into(),
        )
    })
}

// =============================================================================
// ENTRY POINT
// =============================================================================

async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();

    match cli.command {
        // === SETUP ===
        Command::Init { clean } => commands::init::run(cli.config, clean).await,
        Command::Config { action } => commands::config_cmd::run(cli.config, action, cli.format),
        Command::Providers { action } => {
            commands::providers::run(cli.config.clone(), action, cli.format).await
        }
        Command::Use { provider, model } => {
            commands::use_cmd::run(cli.config, provider, model).await
        }
        Command::Reset { force } => commands::reset::run(force, cli.format),

        // === LIFECYCLE ===
        Command::Start => commands::start::run(cli.format).await,
        Command::Stop => commands::stop::run(cli.format).await,
        Command::Restart => commands::restart::run(cli.format).await,
        Command::Status => {
            let timeout = Duration::from_secs(cli.timeout);
            commands::status::run(cli.config.as_deref(), cli.sock, timeout, cli.format).await
        }

        // === DIAGNOSTICS ===
        Command::Doctor => commands::doctor::run(cli.config).await,
        Command::Info => commands::info::run(cli.config).await,

        // === DATA ===
        Command::Frames { action } => commands::frames::run(cli.config, action, cli.format).await,
        Command::Monitor { filter } => commands::monitor::run(cli.config, cli.addr, filter).await,

        // === INTERACTION ===
        Command::Chat { action } => {
            let sock = resolve_sock(cli.sock, cli.config.as_deref())?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::chat::run(&mut client, action, timeout, cli.format).await
        }
        Command::Tui { args } => commands::tui_cmd::run(cli.config, cli.addr, args),

        // === SERVICE ===
        Command::Service { action } => commands::service::run(action, cli.format).await,

        // === EMS ===
        Command::Ems { action } => {
            let sock = resolve_sock(cli.sock, cli.config.as_deref())?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::ems::run(&mut client, action, timeout, cli.format).await
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
