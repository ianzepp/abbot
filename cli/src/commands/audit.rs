//! Audit command - Frame audit log replay and lookup
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Maps `abbot-cli audit replay` and `abbot-cli audit get` to the daemon's
//! `audit.replay` and `audit.get` RPC methods. Replay returns streamed items;
//! get returns a single frame by UUID.

use std::time::Duration;

use clap::Subcommand;
use serde_json::json;

use crate::client::RpcClient;
use crate::error::CliError;
use crate::output::{self, OutputFormat};

#[derive(Debug, Subcommand)]
pub enum AuditAction {
    /// Replay recent frames from the audit log
    Replay {
        /// Maximum number of frames to return
        #[arg(long, default_value = "50")]
        limit: u64,
        /// Filter by syscall name (substring match)
        #[arg(long)]
        name: Option<String>,
        /// Filter by frame op (req, ok, error, etc.)
        #[arg(long)]
        op: Option<String>,
    },
    /// Get a single frame by ID
    Get {
        /// Frame UUID
        id: String,
    },
}

/// Dispatch audit subcommands to the daemon.
///
/// WHY optional params: The daemon handles missing filter fields by applying
/// no filter. We only include name/op in the params when the user provides
/// them, keeping the wire payload minimal.
pub async fn run(
    client: &mut RpcClient,
    action: AuditAction,
    timeout: Duration,
    format: OutputFormat,
) -> Result<(), CliError> {
    match action {
        AuditAction::Replay { limit, name, op } => {
            let mut params = json!({ "limit": limit });
            if let Some(n) = name {
                params["name"] = json!(n);
            }
            if let Some(o) = op {
                params["op"] = json!(o);
            }
            let resp = client.call("audit.replay", params, timeout).await?;
            output::print_response(&resp, format);
        }
        AuditAction::Get { id } => {
            let resp = client
                .call("audit.get", json!({ "id": id }), timeout)
                .await?;
            output::print_response(&resp, format);
        }
    }
    Ok(())
}
