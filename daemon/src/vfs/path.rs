//! VFS Path Utilities - Path Normalization and Expansion
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module provides path manipulation utilities for the VFS system. It
//! handles two critical operations: normalizing guest (VFS) paths to prevent
//! directory traversal attacks, and expanding host paths to support portable
//! configuration with tilde (~) home directory expansion.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Security first: Path normalization prevents directory traversal attacks
//!   by rejecting paths that escape the root via .. components
//! - Lexical processing: Operations are performed on path strings without
//!   touching the filesystem, keeping them fast and side-effect-free
//! - Explicit validation: Paths that don't meet requirements (absolute, etc.)
//!   are rejected with clear error messages
//!
//! SECURITY MODEL
//! ==============
//! - Escape prevention: normalize_path() rejects paths where .. components
//!   would escape the root, preventing access outside mounted directories
//! - Lexical only: No filesystem access during normalization, preventing
//!   TOCTOU (time-of-check-time-of-use) vulnerabilities
//! - Symlink handling: Symlink resolution happens later, in mount::resolve(),
//!   where escape detection can warn but not block (some use cases need it)
//!
//! TRADE-OFFS
//! ==========
//! - No canonicalization: We don't call canonicalize() during normalization,
//!   so symlinks and relative paths aren't fully resolved. This is intentional
//!   to keep normalization fast and avoid filesystem dependencies.
//! - Strict host path requirements: Host paths must start with / or ~, rejecting
//!   relative paths. This prevents ambiguity but requires explicit configuration.

use std::path::{Component, Path, PathBuf};

use crate::kernel::KernelError;

// =============================================================================
// PATH NORMALIZATION
// =============================================================================

/// Normalize a path lexically without touching the filesystem.
///
/// WHY lexical: Avoids filesystem access, making this fast and free of
/// TOCTOU vulnerabilities. The tradeoff is that symlinks aren't resolved,
/// but that's handled later in mount::resolve().
///
/// SECURITY: Rejects paths where .. components escape the root. For example,
/// "/foo/../../bar" is rejected because the second .. tries to go above /.
///
/// Operations performed:
/// - Removes `.` components (current directory references)
/// - Folds `..` components, rejecting paths that escape root
/// - Collapses consecutive slashes (//)
/// - Strips trailing slashes (except for root "/")
pub fn normalize_path(path: &str) -> Result<PathBuf, KernelError> {
    if path.is_empty() {
        return Err(KernelError::invalid_args("path cannot be empty"));
    }

    let p = Path::new(path);
    let mut out = PathBuf::new();

    for c in p.components() {
        match c {
            // WHY skip CurDir: "." doesn't change the path
            Component::CurDir => {}

            // WHY pop for ParentDir: ".." navigates up one level
            Component::ParentDir => {
                if !out.pop() {
                    // WHY reject: Attempting to go above root is a directory
                    // traversal attack (e.g., "/../etc/passwd")
                    return Err(KernelError::forbidden("path escapes root"));
                }
            }

            // WHY push everything else: Root, prefix, and normal components
            // are kept as-is
            other => out.push(other.as_os_str()),
        }
    }

    Ok(out)
}

// =============================================================================
// VFS CWD RESOLUTION
// =============================================================================

/// Resolve a VFS path against a CWD. If the path is absolute (starts with `/`),
/// it is normalized as-is. Otherwise, it is joined with the CWD before normalizing.
///
/// This enables relative paths like `docs/foo.md` and `.` in fs:* syscalls when
/// agents have changed their VFS working directory via `fs:cd`.
pub fn resolve_vfs_path(path: &str, cwd: &str) -> Result<String, KernelError> {
    if path.is_empty() {
        return Err(KernelError::invalid_args("path cannot be empty"));
    }

    // Absolute paths are normalized directly
    if path.starts_with('/') {
        let normalized = normalize_path(path)?;
        return Ok(normalized.to_string_lossy().to_string());
    }

    // Relative paths are joined with CWD
    let joined = if cwd.ends_with('/') {
        format!("{}{}", cwd, path)
    } else {
        format!("{}/{}", cwd, path)
    };

    let normalized = normalize_path(&joined)?;
    Ok(normalized.to_string_lossy().to_string())
}

// =============================================================================
// HOST PATH EXPANSION
// =============================================================================

/// Expand a host path, resolving `~` to the home directory.
///
/// WHY tilde expansion: Allows portable configuration files that work across
/// different users without hardcoding absolute paths.
///
/// SECURITY: Requires paths to start with `~` or `/`, rejecting relative
/// paths like "foo/bar" which would be ambiguous (relative to what?).
///
/// Supported formats:
/// - "~" → user's home directory
/// - "~/foo/bar" → home directory joined with "foo/bar"
/// - "/absolute/path" → unchanged
pub fn expand_host_path(path: &str) -> Result<PathBuf, KernelError> {
    if path.is_empty() {
        return Err(KernelError::invalid_args("host path cannot be empty"));
    }

    // WHY special-case "~": Exact tilde expands to home directory
    if path == "~" {
        return dirs::home_dir()
            .ok_or_else(|| KernelError::internal("cannot expand ~: home directory unknown"));
    }

    // WHY "~/" prefix: Tilde-slash is conventional Unix syntax for home paths
    if let Some(stripped) = path.strip_prefix("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| KernelError::internal("cannot expand ~: home directory unknown"))?;
        return Ok(home.join(stripped));
    }

    // WHY absolute paths pass through: Already unambiguous, no expansion needed
    if path.starts_with('/') {
        return Ok(PathBuf::from(path));
    }

    // WHY reject relative paths: Ambiguous in configuration context. Is
    // "foo/bar" relative to cwd? Binary location? Config file directory?
    // Require explicit absolute or tilde paths to avoid confusion.
    Err(KernelError::invalid_args(
        "host path must start with '~' or '/'",
    ))
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_simple() {
        let p = normalize_path("/foo/bar").unwrap();
        assert_eq!(p, PathBuf::from("/foo/bar"));
    }

    #[test]
    fn test_normalize_removes_dot() {
        let p = normalize_path("/foo/./bar").unwrap();
        assert_eq!(p, PathBuf::from("/foo/bar"));
    }

    #[test]
    fn test_normalize_folds_dotdot() {
        let p = normalize_path("/foo/bar/../baz").unwrap();
        assert_eq!(p, PathBuf::from("/foo/baz"));
    }

    #[test]
    fn test_normalize_collapses_slashes() {
        let p = normalize_path("/foo//bar///baz").unwrap();
        assert_eq!(p, PathBuf::from("/foo/bar/baz"));
    }

    #[test]
    fn test_normalize_escape_rejected() {
        let err = normalize_path("/foo/../../bar").unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[test]
    fn test_normalize_empty_rejected() {
        let err = normalize_path("").unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
    }

    #[test]
    fn test_expand_absolute() {
        let p = expand_host_path("/data/shared").unwrap();
        assert_eq!(p, PathBuf::from("/data/shared"));
    }

    #[test]
    fn test_expand_tilde_only() {
        let p = expand_host_path("~").unwrap();
        assert!(p.is_absolute());
        assert!(!p.to_string_lossy().contains('~'));
    }

    #[test]
    fn test_expand_tilde_prefix() {
        let p = expand_host_path("~/projects/foo").unwrap();
        assert!(p.is_absolute());
        assert!(p.to_string_lossy().ends_with("projects/foo"));
    }

    #[test]
    fn test_expand_relative_rejected() {
        let err = expand_host_path("foo/bar").unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
    }

    #[test]
    fn test_expand_empty_rejected() {
        let err = expand_host_path("").unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
    }

    // =========================================================================
    // resolve_vfs_path tests
    // =========================================================================

    #[test]
    fn test_resolve_absolute_path() {
        let r = resolve_vfs_path("/foo/bar", "/any/cwd").unwrap();
        assert_eq!(r, "/foo/bar");
    }

    #[test]
    fn test_resolve_relative_path() {
        let r = resolve_vfs_path("docs/foo.md", "/projects/abbot").unwrap();
        assert_eq!(r, "/projects/abbot/docs/foo.md");
    }

    #[test]
    fn test_resolve_dot() {
        let r = resolve_vfs_path(".", "/projects/abbot").unwrap();
        assert_eq!(r, "/projects/abbot");
    }

    #[test]
    fn test_resolve_dotdot() {
        let r = resolve_vfs_path("..", "/projects/abbot").unwrap();
        assert_eq!(r, "/projects");
    }

    #[test]
    fn test_resolve_relative_with_dotdot() {
        let r = resolve_vfs_path("../other/file.txt", "/projects/abbot").unwrap();
        assert_eq!(r, "/projects/other/file.txt");
    }

    #[test]
    fn test_resolve_empty_rejected() {
        let err = resolve_vfs_path("", "/cwd").unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARGS");
    }

    #[test]
    fn test_resolve_relative_from_root() {
        let r = resolve_vfs_path("foo/bar", "/").unwrap();
        assert_eq!(r, "/foo/bar");
    }

    #[test]
    fn test_resolve_escape_rejected() {
        let err = resolve_vfs_path("../../..", "/a/b").unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }
}
