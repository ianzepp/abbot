//! Service command - Manage abbot as a system service (launchd/systemd)

use std::path::PathBuf;

use clap::Subcommand;

use crate::error::CliError;

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
}

pub fn run(action: ServiceAction) -> Result<(), CliError> {
    let service_name = "com.abbot.daemon";
    let abbot_bin = std::env::current_exe().map_err(|e| CliError::General(e.to_string()))?;

    // The service should run abbotd, not the CLI. Look for it next to the CLI binary.
    let abbotd_bin = abbot_bin
        .parent()
        .map(|d| d.join("abbotd"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("abbotd"));

    run_inner(action, service_name, &abbotd_bin)
}

fn run_inner(
    action: ServiceAction,
    service_name: &str,
    abbotd_bin: &std::path::Path,
) -> Result<(), CliError> {
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
                println!("Installed service: {}", plist_path.display());
                println!("\nTo start: abbot service start");
            }

            ServiceAction::Uninstall => {
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap_or("")])
                    .output();

                if plist_path.exists() {
                    std::fs::remove_file(&plist_path)?;
                    println!("Uninstalled service: {}", plist_path.display());
                } else {
                    println!("Service not installed");
                }
            }

            ServiceAction::Start => {
                if !plist_path.exists() {
                    println!("Service not installed. Run: abbot service install");
                    return Ok(());
                }

                let output = std::process::Command::new("launchctl")
                    .args(["load", plist_path.to_str().unwrap_or("")])
                    .output()?;

                if output.status.success() {
                    println!("Service started");
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.contains("already loaded") {
                        println!("Service already running");
                    } else {
                        println!("Failed to start: {}", stderr);
                    }
                }
            }

            ServiceAction::Stop => {
                if !plist_path.exists() {
                    println!("Service not installed");
                    return Ok(());
                }

                let output = std::process::Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap_or("")])
                    .output()?;

                if output.status.success() {
                    println!("Service stopped");
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("Failed to stop: {}", stderr);
                }
            }

            ServiceAction::Status => {
                if !plist_path.exists() {
                    println!("Service not installed");
                    return Ok(());
                }

                let output = std::process::Command::new("launchctl")
                    .args(["list", service_name])
                    .output()?;

                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let parts: Vec<&str> = stdout.trim().split_whitespace().collect();
                    if parts.len() >= 3 {
                        let pid = parts[0];
                        let status = parts[1];
                        if pid == "-" {
                            println!("Service: **stopped**");
                            println!("Exit status: {}", status);
                        } else {
                            println!("Service: **running**");
                            println!("PID: {}", pid);
                        }
                    } else {
                        println!("Service: **running**");
                    }
                } else {
                    println!("Service: **stopped** (not loaded)");
                }

                println!("Plist: `{}`", plist_path.display());
                println!("Binary: `{}`", abbotd_bin.display());
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
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

                println!("Installed service: {}", unit_path.display());
                println!("\nTo start: abbot service start");
                println!("To enable on boot: systemctl --user enable abbot");
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

                    println!("Uninstalled service: {}", unit_path.display());
                } else {
                    println!("Service not installed");
                }
            }

            ServiceAction::Start => {
                if !unit_path.exists() {
                    println!("Service not installed. Run: abbot service install");
                    return Ok(());
                }

                let output = std::process::Command::new("systemctl")
                    .args(["--user", "start", "abbot"])
                    .output()?;

                if output.status.success() {
                    println!("Service started");
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("Failed to start: {}", stderr);
                }
            }

            ServiceAction::Stop => {
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "stop", "abbot"])
                    .output()?;

                if output.status.success() {
                    println!("Service stopped");
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("Failed to stop: {}", stderr);
                }
            }

            ServiceAction::Status => {
                if !unit_path.exists() {
                    println!("Service not installed");
                    return Ok(());
                }

                let output = std::process::Command::new("systemctl")
                    .args(["--user", "status", "abbot", "--no-pager"])
                    .output()?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                println!("{}", stdout);
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = action;
        let _ = service_name;
        let _ = abbotd_bin;
        println!("Service management not supported on this platform");
        println!("Supported: macOS (launchd), Linux (systemd)");
    }

    Ok(())
}
