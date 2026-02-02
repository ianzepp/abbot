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
    pub cwd: PathBuf,
}

impl SyscallContext {
    pub fn new(call_id: Uuid, cwd: PathBuf, cancel: CancellationToken) -> Self {
        Self {
            call_id,
            scope: None,
            deadline_ms: None,
            cancel,
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

    /// Returns true if this context has mutation privileges (head scope).
    pub fn can_mutate(&self) -> bool {
        self.scope
            .as_ref()
            .map(|s| s.starts_with("head/"))
            .unwrap_or(false)
    }

    /// Returns Ok(()) if mutation is allowed, Err otherwise.
    pub fn require_mutation(&self) -> Result<(), KernelError> {
        if self.can_mutate() {
            Ok(())
        } else {
            Err(KernelError::forbidden("mutation requires head scope"))
        }
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

    #[test]
    fn test_syscall_context_cancelled() {
        let cancel = CancellationToken::new();
        let ctx = SyscallContext::new(Uuid::new_v4(), PathBuf::from("/tmp"), cancel.clone());

        assert!(!ctx.is_cancelled());
        assert!(ctx.check_cancelled().is_ok());

        cancel.cancel();

        assert!(ctx.is_cancelled());
        let err = ctx.check_cancelled().unwrap_err();
        assert_eq!(err.code, "E_CANCELLED");
    }

    #[test]
    fn test_can_mutate_head_scope() {
        let ctx = SyscallContext::new(
            Uuid::new_v4(),
            PathBuf::from("/tmp"),
            CancellationToken::new(),
        )
        .with_scope(Some("head/abc123".to_string()));

        assert!(ctx.can_mutate());
        assert!(ctx.require_mutation().is_ok());
    }

    #[test]
    fn test_can_mutate_hand_scope() {
        let ctx = SyscallContext::new(
            Uuid::new_v4(),
            PathBuf::from("/tmp"),
            CancellationToken::new(),
        )
        .with_scope(Some("hand/abc123".to_string()));

        assert!(!ctx.can_mutate());
        let err = ctx.require_mutation().unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }

    #[test]
    fn test_can_mutate_no_scope() {
        let ctx = SyscallContext::new(
            Uuid::new_v4(),
            PathBuf::from("/tmp"),
            CancellationToken::new(),
        );

        assert!(!ctx.can_mutate());
        let err = ctx.require_mutation().unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }
}
