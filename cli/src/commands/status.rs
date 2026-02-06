//! Status command - Runtime status inspection
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Maps `abbot-cli status info` to the daemon's `status.info` RPC method.
//! Returns live daemon state: uptime, pool sizes, queue depths, active agents.
//! Complements the static `abbot info` command with runtime data.

use std::time::Duration;

use clap::Subcommand;
use serde_json::json;

use crate::client::RpcClient;
use crate::error::CliError;
use crate::output::{self, OutputFormat};

#[derive(Debug, Subcommand)]
pub enum StatusAction {
    /// Show runtime status (uptime, queues, agents)
    Info,
}

/// Dispatch status subcommands to the daemon.
pub async fn run(
    client: &mut RpcClient,
    action: StatusAction,
    timeout: Duration,
    format: OutputFormat,
) -> Result<(), CliError> {
    match action {
        StatusAction::Info => {
            let resp = client.call("status.info", json!({}), timeout).await?;
            output::print_response(&resp, format);
        }
    }
    Ok(())
}
