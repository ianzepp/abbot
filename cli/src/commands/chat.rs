//! Chat command - Send user messages to the daemon
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Maps `abbot-cli chat send` to the daemon's `chat.send` RPC method.
//! The daemon handles the message with appropriate authority internally
//! (actor="user" is forced by the RPC layer).
//!
//! NOTE: The `--scope` flag defaults to "main", matching the daemon's
//! default scope for user interactions.

use std::time::Duration;

use clap::Subcommand;
use serde_json::json;

use crate::client::RpcClient;
use crate::error::CliError;
use crate::output::{self, OutputFormat};

#[derive(Debug, Subcommand)]
pub enum ChatAction {
    /// Send a chat message
    Send {
        /// The message text
        message: String,
        /// Target scope
        #[arg(long, default_value = "main")]
        scope: String,
    },
}

/// Dispatch chat subcommands to the daemon.
pub async fn run(
    client: &mut RpcClient,
    action: ChatAction,
    timeout: Duration,
    format: OutputFormat,
) -> Result<(), CliError> {
    match action {
        ChatAction::Send { message, scope } => {
            let resp = client
                .call(
                    "chat.send",
                    json!({ "message": message, "scope": scope }),
                    timeout,
                )
                .await?;
            output::print_response(&resp, format);
        }
    }
    Ok(())
}
