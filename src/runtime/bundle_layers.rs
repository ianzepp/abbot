// Shared context layers for bundlers.
//
// These functions produce markdown sections that can be included in any bundler's context.
// Each layer is independent and composable.

use std::path::Path;
use std::process::Command;

/// Build environment context: platform, architecture, time, workspace, git info.
pub fn build_environment_layer(workspace: Option<&Path>) -> String {
    let platform = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let now = chrono::Local::now();
    let build_mode = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    let mut lines = vec![
        "## Environment".to_string(),
        String::new(),
        format!("- Platform: {} ({})", platform, arch),
        format!("- Build: {}", build_mode),
        format!("- Local time: {}", now.format("%Y-%m-%d %H:%M:%S %Z")),
    ];

    if let Some(ws) = workspace {
        lines.push(format!("- Workspace: {}", ws.display()));

        // Git info if workspace is a repo
        if let Some(git_info) = get_git_info(ws) {
            lines.push(format!("- Git: {}", git_info));
        }
    }

    lines.join("\n")
}

/// Build network context: build-time config + host identity.
///
/// Note: this is process-level context. Per-request connection metadata (Host header,
/// remote address) is not available here.
pub fn build_network_layer() -> String {
    let hostname = Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(unknown)".to_string());

    let bind_addr = std::env::var("ABBOT_EFFECTIVE_ADDR")
        .ok()
        .or_else(|| std::env::var("ABBOT_ADDR").ok());

    let mut lines = vec!["## Network".to_string(), String::new()];
    lines.push(format!("- Hostname: {}", hostname));

    let localhost = bind_addr
        .as_deref()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .and_then(|a| a.split(':').next())
        .map(|host| host == "127.0.0.1" || host == "localhost" || host == "[::1]");

    lines.push(format!(
        "- Bind addr: {}",
        bind_addr.as_deref().unwrap_or("(unknown)")
    ));
    lines.push(match localhost {
        Some(true) => "- Localhost: true".to_string(),
        Some(false) => "- Localhost: false".to_string(),
        None => "- Localhost: (unknown)".to_string(),
    });

    lines.join("\n")
}

/// Get git branch, dirty state, and remote URL.
fn get_git_info(workspace: &Path) -> Option<String> {
    // Check if .git exists
    if !workspace.join(".git").exists() {
        return None;
    }

    let branch = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())?;

    // Use diff-index for staged + diff for unstaged; both exit non-zero if changes exist
    let has_staged = Command::new("git")
        .args(["diff-index", "--quiet", "HEAD", "--"])
        .current_dir(workspace)
        .status()
        .ok()
        .map(|s| !s.success())
        .unwrap_or(false);

    let has_unstaged = Command::new("git")
        .args(["diff", "--quiet"])
        .current_dir(workspace)
        .status()
        .ok()
        .map(|s| !s.success())
        .unwrap_or(false);

    let is_dirty = has_staged || has_unstaged;

    let remote = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    let dirty_marker = if is_dirty { " (dirty)" } else { "" };

    match remote {
        Some(url) => Some(format!("{}{} @ {}", branch, dirty_marker, url)),
        None => Some(format!("{}{}", branch, dirty_marker)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn environment_layer_includes_platform() {
        let output = build_environment_layer(None);
        assert!(output.contains("## Environment"));
        assert!(output.contains("Platform:"));
        assert!(output.contains("Build:"));
        assert!(output.contains("Local time:"));
        assert!(!output.contains("Workspace:"));
    }

    #[test]
    fn network_layer_includes_hostname() {
        let output = build_network_layer();
        assert!(output.contains("## Network"));
        assert!(output.contains("Hostname:"));
        assert!(output.contains("Bind addr:"));
    }

    #[test]
    fn environment_layer_includes_workspace() {
        let ws = PathBuf::from("/tmp/test-workspace");
        let output = build_environment_layer(Some(&ws));
        assert!(output.contains("Workspace: /tmp/test-workspace"));
    }

    #[test]
    fn git_info_from_real_repo() {
        // Use the current repo as test fixture
        let ws = std::env::current_dir().unwrap();
        if ws.join(".git").exists() {
            let info = get_git_info(&ws);
            assert!(info.is_some());
            let info = info.unwrap();
            // Should have branch name at minimum
            assert!(!info.is_empty());
        }
    }

    #[test]
    fn git_info_returns_none_for_non_repo() {
        let ws = PathBuf::from("/tmp");
        let info = get_git_info(&ws);
        assert!(info.is_none());
    }
}
