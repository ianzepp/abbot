//! Task command - Inspect the task queue
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Maps `abbot-cli task list` to the daemon's `task.list` RPC method.
//! Returns queued and active tasks as streamed items.

use std::time::Duration;

use clap::Subcommand;
use serde_json::json;

use crate::client::RpcClient;
use crate::error::CliError;
use crate::output::{self, OutputFormat};

#[derive(Debug, Subcommand)]
pub enum TaskAction {
    /// List queued and active tasks
    List,
}

/// Dispatch task subcommands to the daemon.
pub async fn run(
    client: &mut RpcClient,
    action: TaskAction,
    timeout: Duration,
    format: OutputFormat,
) -> Result<(), CliError> {
    match action {
        TaskAction::List => {
            let resp = client.call("task.list", json!({}), timeout).await?;
            output::print_response(&resp, format);
        }
    }
    Ok(())
}
