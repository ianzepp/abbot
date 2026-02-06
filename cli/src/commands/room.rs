//! Room command - Inspect active rooms
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Maps `abbot-cli room list` to the daemon's `room.list` RPC method.
//! Returns active rooms as streamed items.

use std::time::Duration;

use clap::Subcommand;
use serde_json::json;

use crate::client::RpcClient;
use crate::error::CliError;
use crate::output::{self, OutputFormat};

#[derive(Debug, Subcommand)]
pub enum RoomAction {
    /// List active rooms
    List,
}

/// Dispatch room subcommands to the daemon.
pub async fn run(
    client: &mut RpcClient,
    action: RoomAction,
    timeout: Duration,
    format: OutputFormat,
) -> Result<(), CliError> {
    match action {
        RoomAction::List => {
            let resp = client.call("room.list", json!({}), timeout).await?;
            output::print_response(&resp, format);
        }
    }
    Ok(())
}
