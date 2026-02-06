//! Need command - Inspect the need queue
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Maps `abbot-cli need list` to the daemon's `need.list` RPC method.
//! Returns queued and active needs as streamed items.

use std::time::Duration;

use clap::Subcommand;
use serde_json::json;

use crate::client::RpcClient;
use crate::error::CliError;
use crate::output::{self, OutputFormat};

#[derive(Debug, Subcommand)]
pub enum NeedAction {
    /// List queued and active needs
    List,
}

/// Dispatch need subcommands to the daemon.
pub async fn run(
    client: &mut RpcClient,
    action: NeedAction,
    timeout: Duration,
    format: OutputFormat,
) -> Result<(), CliError> {
    match action {
        NeedAction::List => {
            let resp = client.call("need.list", json!({}), timeout).await?;
            output::print_response(&resp, format);
        }
    }
    Ok(())
}
