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
#[command(after_help = "\
Examples:
  abbot init              First-time setup (provider, model, personality)
  abbot init --clean      Wipe config and re-initialize from scratch
  abbot service start     Start the daemon
  abbot service stop      Stop the daemon
  abbot doctor            Run offline health checks")]
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
    /// Chat operations
    Chat {
        #[command(subcommand)]
        action: commands::chat::ChatAction,
    },
    /// Read or write configuration (~/.abbot/abbot.toml)
    Config {
        #[command(subcommand)]
        action: Option<commands::config_cmd::ConfigAction>,
    },
    /// Check system health (offline preflight checks)
    Doctor,
    /// Entity management (needs, tasks, rooms)
    Ems {
        #[command(subcommand)]
        action: commands::ems::EmsAction,
    },
    /// Query frame history (structured, from SQLite)
    Frames {
        #[command(subcommand)]
        action: commands::frames::FramesAction,
    },
    /// Initialize Abbot configuration (first-time setup)
    Init {
        /// Delete ~/.abbot/ entirely before re-initializing
        #[arg(long)]
        clean: bool,
        /// Enable developer/dogfood mode (agents report Abbot issues)
        #[arg(long)]
        developer: bool,
        /// Provider name (anthropic, openai, gemini, xai, zai, openrouter, ollama)
        #[arg(short, long)]
        provider: Option<String>,
        /// Model ID (e.g. claude-sonnet-4-20250514). Defaults to provider's default model
        #[arg(short, long)]
        model: Option<String>,
        /// Skip all interactive prompts, use defaults for traits/cadence/intro
        #[arg(long)]
        accept_defaults: bool,
    },
    /// Manage VFS mount points
    Mounts {
        #[command(subcommand)]
        action: commands::mounts::MountsAction,
    },
    /// Manage model providers
    Providers {
        #[command(subcommand)]
        action: commands::providers::ProvidersAction,
    },
    /// Launch an external tool with Abbot as the API provider
    Run {
        #[command(subcommand)]
        target: commands::run_cmd::RunTarget,
    },
    /// Run diagnostic scripts (offline, developer only)
    Scripts {
        #[command(subcommand)]
        action: commands::scripts::ScriptsAction,
    },
    /// Manage system service (install/uninstall/start/stop/restart)
    Service {
        #[command(subcommand)]
        action: commands::service::ServiceAction,
    },
    /// Show daemon status (service + runtime)
    Status,
    /// Stream live frames (WebSocket)
    Tail {
        /// Filter by kind/name pattern (e.g., "chat:*", "need:*")
        #[arg(long)]
        filter: Option<String>,
        /// Include SIGTICK frames in output
        #[arg(long)]
        show_ticks: bool,
    },
    /// Switch the active LLM provider/model and run preflight
    Use {
        /// Provider name (e.g., anthropic, openai, openrouter, ollama)
        provider: String,
        /// Model name (e.g., claude-sonnet-4-20250514, gpt-4.1)
        model: String,
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
        Command::Chat { action } => {
            let sock = resolve_sock(cli.sock, cli.config.as_deref())?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::chat::run(&mut client, action, timeout, cli.format).await
        }
        Command::Config { action } => commands::config_cmd::run(cli.config, action, cli.format),
        Command::Doctor => commands::doctor::run(cli.config).await,
        Command::Ems { action } => {
            let sock = resolve_sock(cli.sock, cli.config.as_deref())?;
            let timeout = Duration::from_secs(cli.timeout);
            let mut client = RpcClient::connect(&sock).await?;
            commands::ems::run(&mut client, action, timeout, cli.format).await
        }
        Command::Frames { action } => commands::frames::run(cli.config, action, cli.format).await,
        Command::Init {
            clean,
            developer,
            provider,
            model,
            accept_defaults,
        } => {
            commands::init::run(
                cli.config,
                clean,
                developer,
                provider,
                model,
                accept_defaults,
            )
            .await
        }
        Command::Mounts { action } => commands::mounts::run(cli.config, action, cli.format),
        Command::Providers { action } => {
            commands::providers::run(cli.config.clone(), action, cli.format).await
        }
        Command::Run { target } => commands::run_cmd::run(cli.config, target),
        Command::Scripts { action } => {
            config::init_app_config(cli.config.as_deref());
            let developer = abbot::runtime::AppConfig::global()
                .developer
                .unwrap_or(false);
            if !developer {
                return Err(CliError::General(
                    "'abbot scripts' requires developer mode (developer = true in abbot.toml)"
                        .into(),
                ));
            }
            commands::scripts::run(cli.config, action, cli.format).await
        }
        Command::Service { action } => commands::service::run(action, cli.format).await,
        Command::Status => {
            let timeout = Duration::from_secs(cli.timeout);
            commands::status::run(cli.config.as_deref(), cli.sock, timeout, cli.format).await
        }
        Command::Tail { filter, show_ticks } => {
            commands::tail::run(cli.config, cli.addr, filter, show_ticks).await
        }
        Command::Use { provider, model } => {
            commands::use_cmd::run(cli.config, provider, model).await
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
