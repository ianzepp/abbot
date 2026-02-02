use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::error::KernelError;
use super::frame::Frame;

pub struct SyscallContext {
    pub call_id: Uuid,
    pub scope: Option<String>,
    pub deadline_ms: Option<u64>,
    pub cancel: CancellationToken,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
}

impl SyscallContext {
    pub fn new(
        call_id: Uuid,
        workspace_root: PathBuf,
        cwd: PathBuf,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            call_id,
            scope: None,
            deadline_ms: None,
            cancel,
            workspace_root,
            cwd,
        }
    }

    pub fn with_scope(mut self, scope: Option<String>) -> Self {
        self.scope = scope;
        self
    }

    pub fn with_deadline(mut self, deadline_ms: Option<u64>) -> Self {
        self.deadline_ms = deadline_ms;
        self
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn check_cancelled(&self) -> Result<(), KernelError> {
        if self.cancel.is_cancelled() {
            Err(KernelError::cancelled("operation cancelled"))
        } else {
            Ok(())
        }
    }

    /// Validates that a path is within the workspace boundary.
    /// Returns the canonicalized absolute path if valid.
    pub fn validate_path(&self, path: &str) -> Result<PathBuf, KernelError> {
        let target = if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };

        let workspace_canonical = self.workspace_root.canonicalize().map_err(|e| {
            KernelError::internal(format!("workspace root cannot be canonicalized: {e}"))
        })?;

        let canonical = match target.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                // Path doesn't exist yet; check parent
                if let Some(parent) = target.parent() {
                    let parent_canonical = parent.canonicalize().map_err(|e| {
                        KernelError::not_found(format!("parent directory does not exist: {e}"))
                    })?;
                    if !parent_canonical.starts_with(&workspace_canonical) {
                        return Err(KernelError::forbidden(format!(
                            "path escapes workspace: {} is outside {}",
                            target.display(),
                            self.workspace_root.display()
                        )));
                    }
                    return Ok(target);
                }
                return Err(KernelError::not_found("invalid path"));
            }
        };

        if !canonical.starts_with(&workspace_canonical) {
            return Err(KernelError::forbidden(format!(
                "path escapes workspace: {} is outside {}",
                canonical.display(),
                self.workspace_root.display()
            )));
        }

        Ok(canonical)
    }
}

#[async_trait]
pub trait Syscall: Send + Sync {
    fn name(&self) -> &'static str;

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_syscall_context_cancelled() {
        let cancel = CancellationToken::new();
        let ctx = SyscallContext::new(
            Uuid::new_v4(),
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp"),
            cancel.clone(),
        );

        assert!(!ctx.is_cancelled());
        assert!(ctx.check_cancelled().is_ok());

        cancel.cancel();

        assert!(ctx.is_cancelled());
        let err = ctx.check_cancelled().unwrap_err();
        assert_eq!(err.code, "E_CANCELLED");
    }

    #[test]
    fn test_validate_path_within_workspace() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().canonicalize().unwrap();
        let subdir = workspace.join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        let file = subdir.join("test.txt");
        std::fs::write(&file, "test").unwrap();

        let ctx = SyscallContext::new(
            Uuid::new_v4(),
            workspace.clone(),
            subdir.clone(),
            CancellationToken::new(),
        );

        let result = ctx.validate_path("test.txt");
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with(&workspace));
    }

    #[test]
    fn test_validate_path_escape_rejected() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();

        let ctx = SyscallContext::new(
            Uuid::new_v4(),
            workspace.clone(),
            workspace.clone(),
            CancellationToken::new(),
        );

        let result = ctx.validate_path("/etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }
}
