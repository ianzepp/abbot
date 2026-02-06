//! Config - Workspace and socket path resolution
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The CLI needs to find the daemon's RPC socket. The resolution chain is:
//! `--sock` flag > `ABBOT_RPC_SOCK` env > config file. This module handles the
//! config file leg: reading `~/.config/abbot/abbot.toml` and deriving the
//! `<workspace>/rpc.sock` path from its `workspace` field.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Follows the same pattern as `tui/src/main.rs` for consistency across
//!   Abbot's local client tools.
//! - All functions return Option, not Result: a missing or unparseable config
//!   file is not an error here — it just means this resolution source has
//!   nothing to offer, and the caller falls through to the next source.

use std::path::PathBuf;

use serde::Deserialize;

// =============================================================================
// TYPES
// =============================================================================

/// Partial deserialization of `~/.config/abbot/abbot.toml`.
///
/// WHY partial: We only need the `workspace` field. Other fields (providers,
/// plugins, etc.) are irrelevant to socket resolution and ignoring them lets
/// the CLI tolerate config schema changes without breakage.
#[derive(Debug, Deserialize)]
struct AbbotConfigFile {
    workspace: Option<String>,
}

// =============================================================================
// RESOLUTION
// =============================================================================

/// Canonical path to the user's Abbot config file.
fn default_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("abbot").join("abbot.toml"))
}

/// Resolve the workspace directory from `~/.config/abbot/abbot.toml`.
///
/// WHY Option: Config may not exist (first run), may not parse (user error),
/// or may have an empty workspace field. All are non-fatal; the caller
/// should fall through to other resolution sources.
pub fn resolve_workspace() -> Option<PathBuf> {
    let path = default_config_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let cfg: AbbotConfigFile = toml::from_str(&raw).ok()?;
    let ws = cfg.workspace?.trim().to_string();
    if ws.is_empty() {
        return None;
    }
    Some(PathBuf::from(ws))
}

/// Derive the default `rpc.sock` path from the workspace.
///
/// WHY separate from resolve_workspace: Callers that need just the workspace
/// (e.g., for deriving other paths) shouldn't have to strip the socket suffix.
pub fn default_rpc_sock() -> Option<PathBuf> {
    resolve_workspace().map(|ws| ws.join("rpc.sock"))
}
