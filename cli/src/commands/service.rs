//! Service command - Manage abbot as a system service (launchd/systemd)

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;

use crate::error::CliError;
use crate::output::{OutputFormat, print_value};

#[derive(Debug, Subcommand, Clone)]
pub enum ServiceAction {
    /// Install abbot as a system service (launchd on macOS, systemd on Linux)
    Install,
    /// Uninstall the system service
    Uninstall,
    /// Start the service
    Start,
    /// Stop the service
    Stop,
    /// Show service status
    Status,
    /// Show the last preflight check results
    Preflight,
}

pub fn run(action: ServiceAction, format: OutputFormat) -> Result<(), CliError> {
    let service_name = "com.abbot.daemon";
    let abbot_bin = std::env::current_exe().map_err(|e| CliError::General(e.to_string()))?;

    // The service should run abbotd, not the CLI. Look for it next to the CLI binary.
    let abbotd_bin = abbot_bin
        .parent()
        .map(|d| d.join("abbotd"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("abbotd"));

    run_inner(action, service_name, &abbotd_bin, format)
}

fn resolve_preflight_log_path() -> Option<PathBuf> {
    let data_dir = abbot::runtime::app_config::config_dir()?;
    let log_path = data_dir.join("preflight.log");
    if log_path.exists() {
        Some(log_path)
    } else {
        None
    }
}

fn run_inner(
    action: ServiceAction,
    service_name: &str,
    abbotd_bin: &std::path::Path,
    format: OutputFormat,
) -> Result<(), CliError> {
    // Handle platform-independent actions first
    if matches!(action, ServiceAction::Preflight) {
        match resolve_preflight_log_path() {
            Some(path) => {
                let content = std::fs::read_to_string(&path).map_err(|e| {
                    CliError::General(format!("failed to read {}: {}", path.display(), e))
                })?;
                print!("{}", content);
            }
            None => {
                println!("No preflight log found. Run the daemon to generate one.");
            }
        }
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let plist_dir = dirs::home_dir()
            .ok_or_else(|| CliError::General("could not find home directory".into()))?
            .join("Library/LaunchAgents");
        let plist_path = plist_dir.join(format!("{}.plist", service_name));

        match action {
            ServiceAction::Install => {
                std::fs::create_dir_all(&plist_dir)?;

                let plist_content = format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{service_name}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{abbotd_bin}</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/abbot.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/abbot.stderr.log</string>
</dict>
</plist>
"#,
                    service_name = service_name,
                    abbotd_bin = abbotd_bin.display()
                );

                std::fs::write(&plist_path, plist_content)?;

                print_value(
                    &json!({
                        "action": "install",
                        "status": "installed",
                        "path": plist_path.display().to_string(),
                    }),
                    format,
                );
            }

            ServiceAction::Uninstall => {
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap_or("")])
                    .output();

                if plist_path.exists() {
                    std::fs::remove_file(&plist_path)?;
                    print_value(
                        &json!({
                            "action": "uninstall",
                            "status": "uninstalled",
                            "path": plist_path.display().to_string(),
                        }),
                        format,
                    );
                } else {
                    print_value(
                        &json!({
                            "action": "uninstall",
                            "status": "not_installed",
                        }),
                        format,
                    );
                }
            }

            ServiceAction::Start => {
                if !plist_path.exists() {
                    print_value(
                        &json!({
                            "action": "start",
                            "status": "not_installed",
                            "message": "Service not installed. Run: abbot service install",
                        }),
                        format,
                    );
                    return Ok(());
                }

                let output = std::process::Command::new("launchctl")
                    .args(["load", plist_path.to_str().unwrap_or("")])
                    .output()?;

                if output.status.success() {
                    print_value(
                        &json!({
                            "action": "start",
                            "status": "started",
                        }),
                        format,
                    );
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.contains("already loaded") {
                        print_value(
                            &json!({
                                "action": "start",
                                "status": "already_running",
                            }),
                            format,
                        );
                    } else {
                        print_value(
                            &json!({
                                "action": "start",
                                "status": "failed",
                                "error": stderr.trim(),
                            }),
                            format,
                        );
                    }
                }
            }

            ServiceAction::Stop => {
                if !plist_path.exists() {
                    print_value(
                        &json!({
                            "action": "stop",
                            "status": "not_installed",
                        }),
                        format,
                    );
                    return Ok(());
                }

                let output = std::process::Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap_or("")])
                    .output()?;

                if output.status.success() {
                    print_value(
                        &json!({
                            "action": "stop",
                            "status": "stopped",
                        }),
                        format,
                    );
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    print_value(
                        &json!({
                            "action": "stop",
                            "status": "failed",
                            "error": stderr.trim(),
                        }),
                        format,
                    );
                }
            }

            ServiceAction::Status => {
                if !plist_path.exists() {
                    print_value(
                        &json!({
                            "action": "status",
                            "status": "not_installed",
                        }),
                        format,
                    );
                    return Ok(());
                }

                let output = std::process::Command::new("launchctl")
                    .args(["list", service_name])
                    .output()?;

                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let parts: Vec<&str> = stdout.split_whitespace().collect();
                    if parts.len() >= 3 {
                        let pid = parts[0];
                        let exit_status = parts[1];
                        if pid == "-" {
                            let mut status_json = json!({
                                "action": "status",
                                "status": "stopped",
                                "exit_status": exit_status,
                                "path": plist_path.display().to_string(),
                                "binary": abbotd_bin.display().to_string(),
                            });
                            if let Some(log) = resolve_preflight_log_path() {
                                status_json["preflight_log"] =
                                    serde_json::Value::String(log.display().to_string());
                            }
                            print_value(&status_json, format);
                        } else {
                            print_value(
                                &json!({
                                    "action": "status",
                                    "status": "running",
                                    "pid": pid,
                                    "path": plist_path.display().to_string(),
                                    "binary": abbotd_bin.display().to_string(),
                                }),
                                format,
                            );
                        }
                    } else {
                        print_value(
                            &json!({
                                "action": "status",
                                "status": "running",
                                "path": plist_path.display().to_string(),
                                "binary": abbotd_bin.display().to_string(),
                            }),
                            format,
                        );
                    }
                } else {
                    let mut status_json = json!({
                        "action": "status",
                        "status": "stopped",
                        "path": plist_path.display().to_string(),
                        "binary": abbotd_bin.display().to_string(),
                    });
                    if let Some(log) = resolve_preflight_log_path() {
                        status_json["preflight_log"] =
                            serde_json::Value::String(log.display().to_string());
                    }
                    print_value(&status_json, format);
                }
            }

            ServiceAction::Preflight => unreachable!("handled above"),
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = service_name; // systemd unit name is hardcoded
        let systemd_dir = dirs::home_dir()
            .ok_or_else(|| CliError::General("could not find home directory".into()))?
            .join(".config/systemd/user");
        let unit_path = systemd_dir.join("abbot.service");

        match action {
            ServiceAction::Install => {
                std::fs::create_dir_all(&systemd_dir)?;

                let unit_content = format!(
                    r#"[Unit]
Description=Abbot AI Daemon
After=network.target

[Service]
Type=simple
ExecStart={abbotd_bin} run
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
"#,
                    abbotd_bin = abbotd_bin.display()
                );

                std::fs::write(&unit_path, unit_content)?;

                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .output();

                print_value(
                    &json!({
                        "action": "install",
                        "status": "installed",
                        "path": unit_path.display().to_string(),
                    }),
                    format,
                );
            }

            ServiceAction::Uninstall => {
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "stop", "abbot"])
                    .output();
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "disable", "abbot"])
                    .output();

                if unit_path.exists() {
                    std::fs::remove_file(&unit_path)?;

                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "daemon-reload"])
                        .output();

                    print_value(
                        &json!({
                            "action": "uninstall",
                            "status": "uninstalled",
                            "path": unit_path.display().to_string(),
                        }),
                        format,
                    );
                } else {
                    print_value(
                        &json!({
                            "action": "uninstall",
                            "status": "not_installed",
                        }),
                        format,
                    );
                }
            }

            ServiceAction::Start => {
                if !unit_path.exists() {
                    print_value(
                        &json!({
                            "action": "start",
                            "status": "not_installed",
                            "message": "Service not installed. Run: abbot service install",
                        }),
                        format,
                    );
                    return Ok(());
                }

                let output = std::process::Command::new("systemctl")
                    .args(["--user", "start", "abbot"])
                    .output()?;

                if output.status.success() {
                    print_value(
                        &json!({
                            "action": "start",
                            "status": "started",
                        }),
                        format,
                    );
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    print_value(
                        &json!({
                            "action": "start",
                            "status": "failed",
                            "error": stderr.trim(),
                        }),
                        format,
                    );
                }
            }

            ServiceAction::Stop => {
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "stop", "abbot"])
                    .output()?;

                if output.status.success() {
                    print_value(
                        &json!({
                            "action": "stop",
                            "status": "stopped",
                        }),
                        format,
                    );
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    print_value(
                        &json!({
                            "action": "stop",
                            "status": "failed",
                            "error": stderr.trim(),
                        }),
                        format,
                    );
                }
            }

            ServiceAction::Status => {
                if !unit_path.exists() {
                    print_value(
                        &json!({
                            "action": "status",
                            "status": "not_installed",
                        }),
                        format,
                    );
                    return Ok(());
                }

                let output = std::process::Command::new("systemctl")
                    .args(["--user", "status", "abbot", "--no-pager"])
                    .output()?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                let is_active = stdout.contains("active (running)");
                let status = if is_active { "running" } else { "stopped" };

                let mut status_json = json!({
                    "action": "status",
                    "status": status,
                    "path": unit_path.display().to_string(),
                    "binary": abbotd_bin.display().to_string(),
                });
                if !is_active {
                    if let Some(log) = resolve_preflight_log_path() {
                        status_json["preflight_log"] =
                            serde_json::Value::String(log.display().to_string());
                    }
                }
                print_value(&status_json, format);
            }

            ServiceAction::Preflight => unreachable!("handled above"),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = action;
        let _ = service_name;
        let _ = abbotd_bin;
        print_value(
            &json!({
                "status": "unsupported",
                "message": "Service management not supported on this platform. Supported: macOS (launchd), Linux (systemd)",
            }),
            format,
        );
    }

    Ok(())
}
