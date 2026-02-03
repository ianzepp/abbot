//! Git HAL - Hardware Abstraction Layer for Git Operations
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module provides a trait-based abstraction for executing git commands.
//! It wraps the process HAL to provide git-specific defaults and output limits.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Specialized abstraction: Focuses on git execution, not generic process spawning
//! - Bounded output: Enforces reasonable limits on stdout/stderr to prevent memory issues
//! - Delegation: Leverages HalProcess for actual execution while adding git-specific policy
//!
//! TRADE-OFFS
//! ==========
//! - Output limits: 2MB stdout, 512KB stderr may be too small for some operations
//!   but prevents OOM on repositories with massive histories or diff output
//! - No stdin support: Current API doesn't support piping data to git commands;
//!   extend if needed for interactive operations

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;

use super::process::{HalCommandOutput, HalProcess, HalProcessError, HostHalProcess};

// =============================================================================
// TRAIT
// =============================================================================
//
// The HalGit trait provides a focused API for git command execution.
//
// WHY separate from HalProcess: Git operations have specific requirements
// (output limits, error handling) that don't apply to all processes. This
// abstraction encapsulates git-specific policy.

#[async_trait]
pub trait HalGit: Send + Sync {
    /// Execute a git command with the given arguments.
    ///
    /// WHY output limits: Git operations on large repositories can produce
    /// massive output (e.g., `git log --all`, `git diff HEAD~1000`). Bounded
    /// output prevents memory exhaustion.
    async fn run(
        &self,
        cwd: &Path,
        argv: &[String],
        timeout: Option<Duration>,
    ) -> Result<HalCommandOutput, HalProcessError>;
}

// =============================================================================
// HOST IMPLEMENTATION
// =============================================================================
//
// HostHalGit wraps HostHalProcess to provide git command execution with
// sensible defaults for output limits.

#[derive(Debug, Default, Clone)]
pub struct HostHalGit {
    proc: HostHalProcess,
}

#[async_trait]
impl HalGit for HostHalGit {
    async fn run(
        &self,
        cwd: &Path,
        argv: &[String],
        timeout: Option<Duration>,
    ) -> Result<HalCommandOutput, HalProcessError> {
        // WHY 2MB stdout: Accommodates typical git output (status, log, diff)
        // while preventing OOM on pathological cases
        const MAX_STDOUT_BYTES: usize = 2 * 1024 * 1024;

        // WHY 512KB stderr: Error messages are typically small; this limit
        // prevents runaway output from git internals or hooks
        const MAX_STDERR_BYTES: usize = 512 * 1024;

        self.proc
            .run_bounded(
                "git",
                argv,
                cwd,
                None,
                timeout,
                MAX_STDOUT_BYTES,
                MAX_STDERR_BYTES,
                None,
            )
            .await
    }
}
