//! Syscall - Kernel operation interface and context
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Syscalls are the uniform interface for all kernel operations. Every syscall
//! receives a SyscallContext (caller identity, cancellation, cwd) and responds
//! via a frame stream (ok, item, done, error).
//!
//! The refactor establishes:
//! - Syscall names are always <namespace>:<verb> (e.g., chat:message, llm:chat)
//! - Actor in context indicates authorship and permission scope
//! - Syscalls may emit multiple response frames (ok, items, events) before done
//! - Cancellation is graceful: check ctx.is_cancelled() before expensive ops
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Context carries caller identity and lifecycle: Syscalls don't manage cancellation
//! - Stream-based responses: Enable incremental results (thinking, text, tools)
//! - Permission separation: can_mutate() checks actor prefix (head/ vs hand/)

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::error::KernelError;
use super::frame::Frame;

// =============================================================================
// SYSCALL CONTEXT
// =============================================================================

/// Execution context for a syscall invocation.
///
/// WHY this exists: Provides caller identity (actor), cancellation coordination,
/// and execution environment (cwd) without requiring syscalls to manage these
/// concerns directly.
pub struct SyscallContext {
    pub call_id: Uuid,
    pub actor: Option<String>,
    pub deadline_ms: Option<u64>,
    pub cancel: CancellationToken,
    pub cwd: PathBuf,
    /// Virtual filesystem CWD for fs:* syscalls. Defaults to "/".
    /// Updated by fs:cd syscall. Does not affect `cwd` (host path for exec:run, git:run).
    pub vfs_cwd: String,
}

impl SyscallContext {
    pub fn new(call_id: Uuid, cwd: PathBuf, cancel: CancellationToken) -> Self {
        Self {
            call_id,
            actor: None,
            deadline_ms: None,
            cancel,
            cwd,
            vfs_cwd: "/".to_string(),
        }
    }

    pub fn with_vfs_cwd(mut self, vfs_cwd: String) -> Self {
        self.vfs_cwd = vfs_cwd;
        self
    }

    pub fn with_actor(mut self, actor: Option<String>) -> Self {
        self.actor = actor;
        self
    }

    /// Get actor as string, defaulting to hand/anonymous.
    ///
    /// WHY default: Syscalls need a stable actor string for logging/permissions
    /// even when frame.actor is None.
    pub fn actor_str(&self) -> &str {
        self.actor.as_deref().unwrap_or("hand/anonymous")
    }

    pub fn with_deadline(mut self, deadline_ms: Option<u64>) -> Self {
        self.deadline_ms = deadline_ms;
        self
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Check cancellation and return error if cancelled.
    ///
    /// WHY: Provides idiomatic ?-based cancellation checks for syscalls.
    pub fn check_cancelled(&self) -> Result<(), KernelError> {
        if self.cancel.is_cancelled() {
            Err(KernelError::cancelled("operation cancelled"))
        } else {
            Ok(())
        }
    }

    /// Check if this context has mutation privileges.
    ///
    /// WHY: Syscalls that mutate kernel state (needs, tasks, rooms) must
    /// verify caller is a head, not a hand. Heads are trusted; hands execute
    /// user-authored code and must not mutate shared state.
    ///
    /// SECURITY NOTE: Unknown actor prefixes are denied by default to prevent
    /// privilege escalation if a new actor type is introduced.
    pub fn can_mutate(&self) -> bool {
        let actor = self.actor_str();
        if actor.starts_with("head/") {
            return true;
        }
        if actor.starts_with("mind/") {
            return true;
        }
        if actor.starts_with("hand/") {
            return false;
        }
        tracing::warn!(actor = %actor, "unknown actor prefix, denying mutation");
        false
    }

    /// Require mutation privileges or return error.
    ///
    /// WHY: Provides idiomatic ?-based permission checks for mutation syscalls.
    pub fn require_mutation(&self) -> Result<(), KernelError> {
        if self.can_mutate() {
            Ok(())
        } else {
            Err(KernelError::forbidden("mutation requires head role"))
        }
    }
}

// =============================================================================
// SYSCALL TRAIT
// =============================================================================

/// Syscall execution interface.
///
/// WHY async trait: Syscalls may perform I/O (database, LLM, external tools).
///
/// WHY stream-based response (tx): Syscalls emit incremental results (thinking,
/// text deltas, tool calls) before final done/error. The refactor establishes
/// llm:chat as a structured item emitter (not a single ok payload).
#[async_trait]
pub trait Syscall: Send + Sync {
    fn name(&self) -> &'static str;

    /// Execute the syscall and emit response frames.
    ///
    /// WHY Result<(), KernelError>: Syscalls return () on success and emit ok/
    /// item/done frames via tx. Returning Err triggers dispatcher to emit an
    /// error frame. This separates protocol (frame stream) from execution
    /// (Result-based error propagation).
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
        .with_actor(Some("head/abc123".to_string()));

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
        .with_actor(Some("hand/abc123".to_string()));

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

    #[test]
    fn test_can_mutate_mind_scope() {
        let ctx = SyscallContext::new(
            Uuid::new_v4(),
            PathBuf::from("/tmp"),
            CancellationToken::new(),
        )
        .with_actor(Some("mind/main".to_string()));

        assert!(ctx.can_mutate());
        assert!(ctx.require_mutation().is_ok());
    }

    #[test]
    fn test_can_mutate_unknown_scope() {
        let ctx = SyscallContext::new(
            Uuid::new_v4(),
            PathBuf::from("/tmp"),
            CancellationToken::new(),
        )
        .with_actor(Some("unknown/abc123".to_string()));

        assert!(!ctx.can_mutate());
        let err = ctx.require_mutation().unwrap_err();
        assert_eq!(err.code, "E_FORBIDDEN");
    }
}
