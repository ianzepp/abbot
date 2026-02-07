//! EMS command - Entity Management System (needs, tasks, rooms)
//!
//! Groups `need.list`, `task.list`, and `room.list` RPC calls under
//! `abbot ems needs|tasks|rooms`.

use std::time::Duration;

use clap::Subcommand;
use serde_json::json;

use crate::client::RpcClient;
use crate::error::CliError;
use crate::output::{self, OutputFormat};

#[derive(Debug, Subcommand)]
pub enum EmsAction {
    /// List queued and active needs
    Needs,
    /// List queued and active tasks
    Tasks,
    /// List active rooms
    Rooms,
}

pub async fn run(
    client: &mut RpcClient,
    action: EmsAction,
    timeout: Duration,
    format: OutputFormat,
) -> Result<(), CliError> {
    match action {
        EmsAction::Needs => {
            let resp = client.call("need.list", json!({}), timeout).await?;
            output::print_response(&resp, format);
        }
        EmsAction::Tasks => {
            let resp = client.call("task.list", json!({}), timeout).await?;
            output::print_response(&resp, format);
        }
        EmsAction::Rooms => {
            let resp = client.call("room.list", json!({}), timeout).await?;
            output::print_response(&resp, format);
        }
    }
    Ok(())
}
