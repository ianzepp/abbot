//! Service command - Manage abbot as a system service (launchd/systemd)

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;
#[cfg(target_os = "macos")]
use tokio::io::{AsyncReadExt, AsyncSeekExt};

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

pub async fn run(action: ServiceAction, format: OutputFormat) -> Result<(), CliError> {
    let service_name = "com.abbot.daemon";
    let abbot_bin = std::env::current_exe().map_err(|e| CliError::General(e.to_string()))?;

    // The service should run abbotd, not the CLI. Look for it next to the CLI binary.
    let abbotd_bin = abbot_bin
        .parent()
        .map(|d| d.join("abbotd"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("abbotd"));

    run_inner(action, service_name, &abbotd_bin, format).await
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

#[derive(Debug, Clone, Copy, Default)]
struct PreflightBaseline {
    content_hash: Option<u64>,
    modified: Option<std::time::SystemTime>,
    len: Option<u64>,
}

fn preflight_log_path() -> Option<PathBuf> {
    abbot::runtime::app_config::config_dir().map(|d| d.join("preflight.log"))
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn read_preflight_baseline() -> PreflightBaseline {
    let Some(log_path) = preflight_log_path() else {
        return PreflightBaseline::default();
    };

    let (modified, len) = match std::fs::metadata(&log_path) {
        Ok(meta) => (meta.modified().ok(), Some(meta.len())),
        Err(_) => (None, None),
    };

    let content_hash = std::fs::read_to_string(&log_path)
        .ok()
        .map(|s| hash_str(&s));

    PreflightBaseline {
        content_hash,
        modified,
        len,
    }
}

fn parse_preflight_result(content: &str) -> Option<bool> {
    // Match the file header written by daemon preflight.
    if content.contains("# Result: PASS") {
        Some(true)
    } else if content.contains("# Result: FAIL") {
        Some(false)
    } else {
        None
    }
}

fn preflight_warning_fail_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .filter(|l| l.contains(" warning FAIL"))
        .take(5)
        .map(|l| l.to_string())
        .collect()
}

async fn wait_for_preflight(
    started_at: std::time::SystemTime,
    baseline: PreflightBaseline,
) -> Option<bool> {
    let log_path = preflight_log_path()?;

    // Poll for up to 30 seconds (60 x 500ms)
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Detect changes using content hash + metadata (mtime can be coarse).
        let (modified, len) = match std::fs::metadata(&log_path) {
            Ok(meta) => (meta.modified().ok(), Some(meta.len())),
            Err(_) => (None, None),
        };

        let content = std::fs::read_to_string(&log_path).unwrap_or_default();
        let content_hash = if content.is_empty() {
            None
        } else {
            Some(hash_str(&content))
        };

        let changed = content_hash.is_some() && content_hash != baseline.content_hash
            || modified.is_some() && modified != baseline.modified
            || len.is_some() && len != baseline.len;

        if changed {
            if let Some(result) = parse_preflight_result(&content) {
                let _ = started_at; // keep signature stable; no longer used for gating
                return Some(result);
            }
        }
    }

    // Timed out: best-effort parse whatever is on disk.
    let content = std::fs::read_to_string(&log_path).ok()?;
    parse_preflight_result(&content)
}

#[cfg(target_os = "macos")]
fn launchd_log_paths() -> (PathBuf, PathBuf) {
    (
        PathBuf::from("/tmp/abbot.stdout.log"),
        PathBuf::from("/tmp/abbot.stderr.log"),
    )
}

#[cfg(target_os = "macos")]
fn launchd_running_pid(service_name: &str) -> Option<Option<String>> {
    // Returns:
    // - Some(Some(pid)) when running
    // - Some(None) when loaded but not running (pid is "-")
    // - None when not loaded / unknown
    let output = std::process::Command::new("launchctl")
        .args(["list", service_name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let pid = parts[0];
    if pid == "-" {
        Some(None)
    } else {
        Some(Some(pid.to_string()))
    }
}

#[cfg(target_os = "macos")]
async fn tail_file_to_stderr(path: PathBuf, stop: tokio::sync::oneshot::Receiver<()>) {
    use std::io::SeekFrom;

    let mut stop = stop;

    // Start tailing from the current end of file to avoid replaying old logs.
    let mut offset: u64 = match std::fs::metadata(&path) {
        Ok(m) => m.len(),
        Err(_) => 0,
    };

    loop {
        tokio::select! {
            _ = &mut stop => {
                break;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => {
                let Ok(mut f) = tokio::fs::File::open(&path).await else {
                    continue;
                };
                if f.seek(SeekFrom::Start(offset)).await.is_err() {
                    continue;
                }
                let mut buf = Vec::new();
                if f.read_to_end(&mut buf).await.is_err() {
                    continue;
                }
                if buf.is_empty() {
                    continue;
                }
                offset = offset.saturating_add(buf.len() as u64);
                eprint!("{}", String::from_utf8_lossy(&buf));
            }
        }
    }
}

async fn run_inner(
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
        let resolved_format = format.resolve();
        let plist_dir = dirs::home_dir()
            .ok_or_else(|| CliError::General("could not find home directory".into()))?
            .join("Library/LaunchAgents");
        let plist_path = plist_dir.join(format!("{}.plist", service_name));

        // If the user blew away ~/.abbot/, fail fast with a clear message instead of
        // relying on launchd KeepAlive restart loops.
        if matches!(action, ServiceAction::Start) {
            let cfg_path = abbot::runtime::app_config::default_config_path();
            if cfg_path.as_ref().is_none_or(|p| !p.exists()) {
                print_value(
                    &json!({
                        "action": "start",
                        "status": "not_configured",
                        "message": "No ~/.abbot/abbot.toml found. Run: abbot init",
                    }),
                    format,
                );
                return Ok(());
            }
        }

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

                // If the agent is already running, avoid waiting for a new preflight log.
                if let Some(Some(pid)) = launchd_running_pid(service_name) {
                    // If the config was deleted while the daemon is running, surface it.
                    let cfg_missing = abbot::runtime::app_config::default_config_path()
                        .as_ref()
                        .is_none_or(|p| !p.exists());
                    if cfg_missing {
                        print_value(
                            &json!({
                                "action": "start",
                                "status": "running_but_unconfigured",
                                "pid": pid,
                                "message": "Daemon is running but ~/.abbot/abbot.toml is missing. Stop and re-init: abbot service stop; abbot init; abbot service start",
                            }),
                            format,
                        );
                        return Ok(());
                    }
                    print_value(
                        &json!({
                            "action": "start",
                            "status": "already_running",
                            "pid": pid,
                        }),
                        format,
                    );
                    return Ok(());
                }

                let started_at = std::time::SystemTime::now();
                let baseline = read_preflight_baseline();
                let output = std::process::Command::new("launchctl")
                    .args(["load", plist_path.to_str().unwrap_or("")])
                    .output()?;

                if output.status.success() {
                    // launchctl may return 0 even if the job is already loaded.
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.contains("already loaded") {
                        print_value(
                            &json!({
                                "action": "start",
                                "status": "already_running",
                            }),
                            format,
                        );
                        return Ok(());
                    }

                    // Provide visible progress in pretty mode by tailing launchd logs
                    // while we wait for the daemon to finish preflight.
                    #[cfg(target_os = "macos")]
                    let (log_stop_tx, log_task) = if matches!(resolved_format, OutputFormat::Pretty)
                    {
                        let (stdout_log, stderr_log) = launchd_log_paths();
                        let (tx1, rx1) = tokio::sync::oneshot::channel::<()>();
                        let (tx2, rx2) = tokio::sync::oneshot::channel::<()>();
                        // New line so log tail doesn't run into the status text.
                        eprintln!("Starting (waiting for preflight)...");
                        let task1 = tokio::spawn(tail_file_to_stderr(stdout_log, rx1));
                        let task2 = tokio::spawn(tail_file_to_stderr(stderr_log, rx2));
                        (Some((tx1, tx2)), Some((task1, task2)))
                    } else {
                        eprint!("Starting...");
                        (None, None)
                    };

                    match wait_for_preflight(started_at, baseline).await {
                        Some(true) => {
                            #[cfg(target_os = "macos")]
                            if let Some((tx1, tx2)) = log_stop_tx {
                                let _ = tx1.send(());
                                let _ = tx2.send(());
                            }
                            #[cfg(target_os = "macos")]
                            if let Some((t1, t2)) = log_task {
                                let _ = t1.await;
                                let _ = t2.await;
                            }
                            if matches!(resolved_format, OutputFormat::Pretty) {
                                eprintln!("preflight ok");

                                if let Some(path) = resolve_preflight_log_path() {
                                    if let Ok(content) = std::fs::read_to_string(&path) {
                                        let warns = preflight_warning_fail_lines(&content);
                                        if !warns.is_empty() {
                                            eprintln!("preflight warnings:");
                                            for l in warns {
                                                eprintln!("{l}");
                                            }
                                            eprintln!("Tip: abbot service preflight");
                                        }
                                    }
                                }
                            } else {
                                eprintln!(" ok");
                            }
                            print_value(
                                &json!({
                                    "action": "start",
                                    "status": "started",
                                }),
                                format,
                            );
                        }
                        Some(false) => {
                            #[cfg(target_os = "macos")]
                            if let Some((tx1, tx2)) = log_stop_tx {
                                let _ = tx1.send(());
                                let _ = tx2.send(());
                            }
                            #[cfg(target_os = "macos")]
                            if let Some((t1, t2)) = log_task {
                                let _ = t1.await;
                                let _ = t2.await;
                            }
                            if matches!(resolved_format, OutputFormat::Pretty) {
                                eprintln!("preflight failed");
                            } else {
                                eprintln!(" preflight failed");
                            }
                            print_value(
                                &json!({
                                    "action": "start",
                                    "status": "preflight_failed",
                                    "message": "Preflight checks failed. Run: abbot service preflight",
                                }),
                                format,
                            );
                        }
                        None => {
                            #[cfg(target_os = "macos")]
                            if let Some((tx1, tx2)) = log_stop_tx {
                                let _ = tx1.send(());
                                let _ = tx2.send(());
                            }
                            #[cfg(target_os = "macos")]
                            if let Some((t1, t2)) = log_task {
                                let _ = t1.await;
                                let _ = t2.await;
                            }
                            if matches!(resolved_format, OutputFormat::Pretty) {
                                // If the report exists, show the tail so it's actionable.
                                if let Some(path) = resolve_preflight_log_path() {
                                    if let Ok(content) = std::fs::read_to_string(&path) {
                                        if let Some(result) = parse_preflight_result(&content) {
                                            let status = if result { "PASS" } else { "FAIL" };
                                            eprintln!(
                                                "preflight result present ({status}) but watcher timed out"
                                            );
                                            let warns = preflight_warning_fail_lines(&content);
                                            if !warns.is_empty() {
                                                eprintln!("preflight warnings:");
                                                for l in warns {
                                                    eprintln!("{l}");
                                                }
                                                eprintln!("Tip: abbot service preflight");
                                            }
                                        } else {
                                            eprintln!("timeout waiting for preflight");
                                        }
                                    } else {
                                        eprintln!("timeout waiting for preflight");
                                    }
                                } else {
                                    eprintln!("timeout waiting for preflight");
                                }
                            } else {
                                eprintln!(" timeout");
                            }
                            print_value(
                                &json!({
                                    "action": "start",
                                    "status": "timeout",
                                    "message": "Timed out waiting for preflight. The daemon may still be starting.",
                                }),
                                format,
                            );
                        }
                    }
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

                let started_at = std::time::SystemTime::now();
                let baseline = read_preflight_baseline();
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "start", "abbot"])
                    .output()?;

                if output.status.success() {
                    eprint!("Starting...");
                    match wait_for_preflight(started_at, baseline).await {
                        Some(true) => {
                            eprintln!(" ok");
                            print_value(
                                &json!({
                                    "action": "start",
                                    "status": "started",
                                }),
                                format,
                            );
                        }
                        Some(false) => {
                            eprintln!(" preflight failed");
                            print_value(
                                &json!({
                                    "action": "start",
                                    "status": "preflight_failed",
                                    "message": "Preflight checks failed. Run: abbot service preflight",
                                }),
                                format,
                            );
                        }
                        None => {
                            eprintln!(" timeout");
                            print_value(
                                &json!({
                                    "action": "start",
                                    "status": "timeout",
                                    "message": "Timed out waiting for preflight. The daemon may still be starting.",
                                }),
                                format,
                            );
                        }
                    }
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
