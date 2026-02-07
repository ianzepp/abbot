//! Status command - Hybrid service + runtime status
//!
//! First checks service-level status (installed, running, pid) via
//! `service::check_service_status()` which works offline. Then, if the
//! daemon is running, attempts an RPC `status.info` call for runtime details.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use crate::client::RpcClient;
use crate::error::CliError;
use crate::output::{self, OutputFormat, print_value};

use super::service;

pub async fn run(
    cli_config: Option<&std::path::Path>,
    sock: Option<PathBuf>,
    timeout: Duration,
    format: OutputFormat,
) -> Result<(), CliError> {
    let svc = service::check_service_status();

    let mut status_json = json!({
        "installed": svc.installed,
        "running": svc.running,
    });

    if let Some(pid) = &svc.pid {
        status_json["pid"] = json!(pid);
    }
    if let Some(exit_status) = &svc.exit_status {
        status_json["exit_status"] = json!(exit_status);
    }
    if let Some(path) = &svc.service_path {
        status_json["service_path"] = json!(path);
    }
    if let Some(path) = &svc.binary_path {
        status_json["binary_path"] = json!(path);
    }
    if let Some(log) = &svc.preflight_log {
        status_json["preflight_log"] = json!(log);
    }

    // If running, attempt RPC for runtime details
    if svc.running {
        let sock_path = resolve_sock_for_status(sock, cli_config);
        if let Some(sock_path) = sock_path {
            match RpcClient::connect(&sock_path).await {
                Ok(mut client) => {
                    match client.call("status.info", json!({}), timeout).await {
                        Ok(resp) => {
                            // In JSON mode, merge runtime into status. In pretty mode, print both.
                            let resolved = format.resolve();
                            match resolved {
                                OutputFormat::Json => {
                                    if let Some(ok_data) = &resp.ok_data {
                                        status_json["runtime"] = ok_data.clone();
                                    }
                                    print_value(&status_json, format);
                                }
                                OutputFormat::Pretty => {
                                    println!("## Service\n");
                                    print_value(&status_json, format);
                                    println!("\n## Runtime\n");
                                    output::print_response(&resp, format);
                                }
                                OutputFormat::Auto => unreachable!(),
                            }
                            return Ok(());
                        }
                        Err(_) => {
                            status_json["runtime"] = json!("unavailable");
                        }
                    }
                }
                Err(_) => {
                    status_json["runtime"] = json!("unavailable");
                }
            }
        }
    }

    print_value(&status_json, format);
    Ok(())
}

fn resolve_sock_for_status(
    sock: Option<PathBuf>,
    cli_config: Option<&std::path::Path>,
) -> Option<PathBuf> {
    if let Some(path) = sock {
        return Some(path);
    }
    if let Ok(path) = std::env::var("ABBOT_RPC_SOCK") {
        return Some(PathBuf::from(path));
    }
    crate::config::default_rpc_sock(cli_config)
}
