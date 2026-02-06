//! Workspace sandboxing and tool-result types for plugin execution.
//!
//! Tool dispatch now lives in `syscalls::dispatch`. This module retains
//! `Workspace` (path sandboxing), `SharedCwd`, `ToolError`, and the
//! `ok()`/`err()` response helpers used by the plugin system.

use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Shared mutable working directory for plugin tool execution.
pub type SharedCwd = Arc<Mutex<PathBuf>>;

// ---------------------------------------------------------------------------
// Workspace sandboxing
// ---------------------------------------------------------------------------

/// Workspace sandbox for tool execution.
///
/// All paths are resolved relative to the workspace root and validated to stay
/// within bounds. Lexical normalization (no symlink resolution) prevents TOCTOU
/// attacks.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a user-provided path relative to cwd, with workspace boundary enforcement.
    pub fn resolve_from_cwd(&self, cwd: &Path, path: &str) -> Result<PathBuf, ToolError> {
        if path.trim().is_empty() {
            return Err(ToolError::invalid_args("path is empty"));
        }

        let expanded = if path.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(&path[2..])
            } else {
                return Err(ToolError::invalid_args(
                    "cannot expand ~: home directory unknown",
                ));
            }
        } else if path == "~" {
            dirs::home_dir()
                .ok_or_else(|| ToolError::invalid_args("cannot expand ~: home directory unknown"))?
        } else {
            PathBuf::from(path)
        };

        let joined = if expanded.is_absolute() {
            if !expanded.starts_with(&self.root) {
                return Err(ToolError::outside_workspace(format!(
                    "path {} is outside workspace {}",
                    expanded.display(),
                    self.root.display()
                )));
            }
            expanded
        } else {
            cwd.join(&expanded)
        };

        let normalized = normalize_no_symlinks(&joined);

        if !normalized.starts_with(&self.root) {
            return Err(ToolError::outside_workspace("path escapes workspace"));
        }

        Ok(normalized)
    }
}

/// Normalize path lexically without following symlinks.
fn normalize_no_symlinks(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tool error types
// ---------------------------------------------------------------------------

/// Tool execution error with structured error codes.
#[derive(Debug, Clone, Serialize)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl ToolError {
    pub fn invalid_args(msg: impl Into<String>) -> Self {
        Self {
            code: "E_INVALID_ARGS".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: "E_NOT_FOUND".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn io(msg: impl Into<String>) -> Self {
        Self {
            code: "E_IO".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn outside_workspace(msg: impl Into<String>) -> Self {
        Self {
            code: "E_OUTSIDE_WORKSPACE".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn patch_failed(msg: impl Into<String>) -> Self {
        Self {
            code: "E_PATCH_FAILED".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self {
            code: "E_FORBIDDEN".to_string(),
            message: msg.into(),
            detail: None,
        }
    }

    pub fn db(msg: impl Into<String>) -> Self {
        Self {
            code: "E_DB".to_string(),
            message: msg.into(),
            detail: None,
        }
    }
}

/// Create a successful tool response JSON string.
pub fn ok(data: Value) -> String {
    json!({"ok": true, "data": data}).to_string()
}

/// Create an error tool response JSON string.
pub fn err(e: ToolError) -> String {
    json!({"ok": false, "error": e}).to_string()
}
