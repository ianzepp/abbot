//! Service command - Manage abbot as a system service (launchd/systemd)
//!
//! Exposes `start_service`, `stop_service`, and `check_service_status` as public
//! functions so that top-level CLI commands (`abbot start`, `abbot stop`,
//! `abbot status`, `abbot restart`) can delegate here. The `ServiceAction` enum
//! is reduced to Install/Uninstall only.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::json;
#[cfg(target_os = "macos")]
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::error::CliError;
use crate::output::{OutputFormat, print_value};

// =============================================================================
// PUBLIC TYPES
// =============================================================================

#[derive(Debug, Subcommand, Clone)]
pub enum ServiceAction {
    /// Install abbot as a system service (launchd on macOS, systemd on Linux)
    Install,
    /// Uninstall the system service
    Uninstall,
}

/// Service status information returned by `check_service_status`.
#[derive(Debug, Clone)]
pub struct ServiceStatus {
    pub installed: bool,
    pub running: bool,
    pub pid: Option<String>,
    pub exit_status: Option<String>,
    pub service_path: Option<String>,
    pub binary_path: Option<String>,
    pub preflight_log: Option<String>,
}

// =============================================================================
// PUBLIC FUNCTIONS
// =============================================================================

/// Resolve the path to the `abbotd` binary (next to the current CLI binary).
pub fn resolve_abbotd_bin() -> PathBuf {
    let abbot_bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("abbot"));
    abbot_bin
        .parent()
        .map(|d| d.join("abbotd"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("abbotd"))
}

/// Start the daemon service. Delegates to platform-specific logic.
pub async fn start_service(format: OutputFormat) -> Result<(), CliError> {
    let service_name = "com.abbot.daemon";
    let abbotd_bin = resolve_abbotd_bin();
    run_start(service_name, &abbotd_bin, format).await
}

/// Stop the daemon service. Delegates to platform-specific logic.
pub async fn stop_service(format: OutputFormat) -> Result<(), CliError> {
    let service_name = "com.abbot.daemon";
    let abbotd_bin = resolve_abbotd_bin();
    run_stop(service_name, &abbotd_bin, format).await
}

/// Check service status without requiring a running daemon.
pub fn check_service_status() -> ServiceStatus {
    let service_name = "com.abbot.daemon";
    let abbotd_bin = resolve_abbotd_bin();
    check_status_inner(service_name, &abbotd_bin)
}

/// Dispatch Install/Uninstall subcommands.
pub async fn run(action: ServiceAction, format: OutputFormat) -> Result<(), CliError> {
    let service_name = "com.abbot.daemon";
    let abbotd_bin = resolve_abbotd_bin();
    run_install_uninstall(action, service_name, &abbotd_bin, format).await
}

// =============================================================================
// PREFLIGHT HELPERS
// =============================================================================

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
    if content.contains("# Preflight: PASS") {
        Some(true)
    } else if content.contains("# Preflight: FAIL") {
        Some(false)
    } else {
        None
    }
}

fn preflight_warning_fail_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .filter(|l| l.starts_with("[WARN]") || l.starts_with("[FAIL]"))
        .take(5)
        .map(|l| l.to_string())
        .collect()
}

async fn wait_for_preflight(
    started_at: std::time::SystemTime,
    baseline: PreflightBaseline,
) -> Option<bool> {
    let log_path = preflight_log_path()?;

    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

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

        if changed && let Some(result) = parse_preflight_result(&content) {
            let _ = started_at;
            return Some(result);
        }
    }

    let content = std::fs::read_to_string(&log_path).ok()?;
    parse_preflight_result(&content)
}

// =============================================================================
// PLATFORM HELPERS (macOS)
// =============================================================================

#[cfg(target_os = "macos")]
fn launchd_log_paths() -> (PathBuf, PathBuf) {
    (
        PathBuf::from("/tmp/abbot.stdout.log"),
        PathBuf::from("/tmp/abbot.stderr.log"),
    )
}

#[cfg(target_os = "macos")]
fn launchd_running_pid(service_name: &str) -> Option<Option<String>> {
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

// =============================================================================
// PLATFORM HELPERS (paths)
// =============================================================================

#[cfg(target_os = "macos")]
fn plist_path(service_name: &str) -> Result<PathBuf, CliError> {
    let plist_dir = dirs::home_dir()
        .ok_or_else(|| CliError::General("could not find home directory".into()))?
        .join("Library/LaunchAgents");
    Ok(plist_dir.join(format!("{}.plist", service_name)))
}

#[cfg(target_os = "linux")]
fn unit_path() -> Result<PathBuf, CliError> {
    let systemd_dir = dirs::home_dir()
        .ok_or_else(|| CliError::General("could not find home directory".into()))?
        .join(".config/systemd/user");
    Ok(systemd_dir.join("abbot.service"))
}

// =============================================================================
// START
// =============================================================================

async fn run_start(
    service_name: &str,
    _abbotd_bin: &std::path::Path,
    format: OutputFormat,
) -> Result<(), CliError> {
    #[cfg(target_os = "macos")]
    {
        let _ = _abbotd_bin;
        let resolved_format = format.resolve();
        let plist = plist_path(service_name)?;

        // Fail fast if config is missing
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

        if !plist.exists() {
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

        // Already running?
        if let Some(Some(pid)) = launchd_running_pid(service_name) {
            let cfg_missing = abbot::runtime::app_config::default_config_path()
                .as_ref()
                .is_none_or(|p| !p.exists());
            if cfg_missing {
                print_value(
                    &json!({
                        "action": "start",
                        "status": "running_but_unconfigured",
                        "pid": pid,
                        "message": "Daemon is running but ~/.abbot/abbot.toml is missing. Stop and re-init: abbot stop; abbot init; abbot start",
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
            .args(["load", plist.to_str().unwrap_or("")])
            .output()?;

        if output.status.success() {
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

            #[cfg(target_os = "macos")]
            let (log_stop_tx, log_task) = if matches!(resolved_format, OutputFormat::Pretty) {
                let (stdout_log, stderr_log) = launchd_log_paths();
                let (tx1, rx1) = tokio::sync::oneshot::channel::<()>();
                let (tx2, rx2) = tokio::sync::oneshot::channel::<()>();
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
                        if let Some(path) = resolve_preflight_log_path()
                            && let Ok(content) = std::fs::read_to_string(&path)
                        {
                            let warns = preflight_warning_fail_lines(&content);
                            if !warns.is_empty() {
                                eprintln!("preflight warnings:");
                                for l in warns {
                                    eprintln!("{l}");
                                }
                                eprintln!("Tip: abbot doctor");
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
                            "message": "Preflight checks failed. Run: abbot doctor",
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
                                        eprintln!("Tip: abbot doctor");
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

    #[cfg(target_os = "linux")]
    {
        let _ = service_name;
        let _ = _abbotd_bin;
        let unit = unit_path()?;

        if !unit.exists() {
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
                            "message": "Preflight checks failed. Run: abbot doctor",
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

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = service_name;
        let _ = _abbotd_bin;
        print_value(
            &json!({
                "status": "unsupported",
                "message": "Service management not supported on this platform.",
            }),
            format,
        );
    }

    Ok(())
}

// =============================================================================
// STOP
// =============================================================================

async fn run_stop(
    service_name: &str,
    _abbotd_bin: &std::path::Path,
    format: OutputFormat,
) -> Result<(), CliError> {
    #[cfg(target_os = "macos")]
    {
        let _ = _abbotd_bin;
        let plist = plist_path(service_name)?;

        if !plist.exists() {
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
            .args(["unload", plist.to_str().unwrap_or("")])
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

    #[cfg(target_os = "linux")]
    {
        let _ = service_name;
        let _ = _abbotd_bin;
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

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = service_name;
        let _ = _abbotd_bin;
        print_value(
            &json!({
                "status": "unsupported",
                "message": "Service management not supported on this platform.",
            }),
            format,
        );
    }

    Ok(())
}

// =============================================================================
// STATUS CHECK
// =============================================================================

fn check_status_inner(service_name: &str, abbotd_bin: &std::path::Path) -> ServiceStatus {
    let mut status = ServiceStatus {
        installed: false,
        running: false,
        pid: None,
        exit_status: None,
        service_path: None,
        binary_path: Some(abbotd_bin.display().to_string()),
        preflight_log: resolve_preflight_log_path().map(|p| p.display().to_string()),
    };

    #[cfg(target_os = "macos")]
    {
        let plist_dir = match dirs::home_dir() {
            Some(h) => h.join("Library/LaunchAgents"),
            None => return status,
        };
        let plist = plist_dir.join(format!("{}.plist", service_name));
        status.service_path = Some(plist.display().to_string());

        if !plist.exists() {
            return status;
        }
        status.installed = true;

        if let Some(pid_opt) = launchd_running_pid(service_name) {
            match pid_opt {
                Some(pid) => {
                    status.running = true;
                    status.pid = Some(pid);
                }
                None => {
                    // Loaded but not running — get exit status
                    let output = std::process::Command::new("launchctl")
                        .args(["list", service_name])
                        .output()
                        .ok();
                    if let Some(out) = output {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        let parts: Vec<&str> = stdout.split_whitespace().collect();
                        if parts.len() >= 2 {
                            status.exit_status = Some(parts[1].to_string());
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = service_name;
        let systemd_dir = match dirs::home_dir() {
            Some(h) => h.join(".config/systemd/user"),
            None => return status,
        };
        let unit = systemd_dir.join("abbot.service");
        status.service_path = Some(unit.display().to_string());

        if !unit.exists() {
            return status;
        }
        status.installed = true;

        let output = std::process::Command::new("systemctl")
            .args(["--user", "status", "abbot", "--no-pager"])
            .output()
            .ok();
        if let Some(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            status.running = stdout.contains("active (running)");
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = service_name;
    }

    status
}

// =============================================================================
// INSTALL / UNINSTALL
// =============================================================================

async fn run_install_uninstall(
    action: ServiceAction,
    service_name: &str,
    abbotd_bin: &std::path::Path,
    format: OutputFormat,
) -> Result<(), CliError> {
    #[cfg(target_os = "macos")]
    {
        let plist_dir = dirs::home_dir()
            .ok_or_else(|| CliError::General("could not find home directory".into()))?
            .join("Library/LaunchAgents");
        let plist = plist_dir.join(format!("{}.plist", service_name));

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

                std::fs::write(&plist, plist_content)?;

                print_value(
                    &json!({
                        "action": "install",
                        "status": "installed",
                        "path": plist.display().to_string(),
                    }),
                    format,
                );
            }

            ServiceAction::Uninstall => {
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", plist.to_str().unwrap_or("")])
                    .output();

                if plist.exists() {
                    std::fs::remove_file(&plist)?;
                    print_value(
                        &json!({
                            "action": "uninstall",
                            "status": "uninstalled",
                            "path": plist.display().to_string(),
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
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = service_name;
        let systemd_dir = dirs::home_dir()
            .ok_or_else(|| CliError::General("could not find home directory".into()))?
            .join(".config/systemd/user");
        let unit = systemd_dir.join("abbot.service");

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

                std::fs::write(&unit, unit_content)?;

                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .output();

                print_value(
                    &json!({
                        "action": "install",
                        "status": "installed",
                        "path": unit.display().to_string(),
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

                if unit.exists() {
                    std::fs::remove_file(&unit)?;

                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "daemon-reload"])
                        .output();

                    print_value(
                        &json!({
                            "action": "uninstall",
                            "status": "uninstalled",
                            "path": unit.display().to_string(),
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
