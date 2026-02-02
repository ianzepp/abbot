use std::path::{Component, Path, PathBuf};

use crate::kernel::KernelError;

/// Normalize a path lexically without touching the filesystem.
/// Removes `.` components, folds `..` components, collapses consecutive slashes,
/// and strips trailing slashes.
pub fn normalize_path(path: &str) -> Result<PathBuf, KernelError> {
    if path.is_empty() {
        return Err(KernelError::invalid_args("path cannot be empty"));
    }

    let p = Path::new(path);
    let mut out = PathBuf::new();

    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(KernelError::forbidden("path escapes root"));
                }
            }
            other => out.push(other.as_os_str()),
        }
    }

    Ok(out)
}

/// Expand a host path, resolving `~` to the home directory.
/// Requires paths to start with `~` or `/`.
pub fn expand_host_path(path: &str) -> Result<PathBuf, KernelError> {
    if path.is_empty() {
        return Err(KernelError::invalid_args("host path cannot be empty"));
    }

    if path == "~" {
        return dirs::home_dir()
            .ok_or_else(|| KernelError::internal("cannot expand ~: home directory unknown"));
    }

    if path.starts_with("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| KernelError::internal("cannot expand ~: home directory unknown"))?;
        return Ok(home.join(&path[2..]));
    }

    if path.starts_with('/') {
        return Ok(PathBuf::from(path));
    }

    Err(KernelError::invalid_args(
        "host path must start with '~' or '/'",
    ))
}

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
}
