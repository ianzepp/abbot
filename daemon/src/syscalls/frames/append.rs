//! Frames:Append - Emit arbitrary frames into the audit trail
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall allows actors to emit custom frames into the kernel's frame store,
//! which are then automatically persisted to `frames.db` by the FrameStore's writer
//! thread. Unlike most syscalls (which emit frames as part of their execution), this
//! syscall provides a direct API for logging custom events.
//!
//! **Primary use case**: Agent logging during execution. For example, a "head" agent
//! might emit progress markers, debugging breadcrumbs, or custom event types that
//! need to be preserved in the audit trail for later analysis.
//!
//! **Frame protocol:**
//! 1. Emit `Frame::event` with structured payload (kind, scope, data)
//! 2. Return `Frame::ok` confirming the event was emitted
//!
//! **Persistence flow:**
//! - Emitted frames are captured by the kernel dispatcher and forwarded to FrameStore
//! - FrameStore's writer thread persists them to SQLite with monotonic sequence numbers
//! - Frames are queryable via `frames:select` immediately after this syscall returns
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Fail-safe logging**: Never fail a syscall due to logging infrastructure issues.
//!   If the frame channel is full or disconnected, we still return success to avoid
//!   cascading failures from audit infrastructure problems.
//! - **Structured metadata**: Require `kind` and `scope` to ensure frames are queryable
//!   and discoverable. Arbitrary `data` allows flexibility for custom payloads.
//! - **No permission checks**: Any actor can log (unlike mutation syscalls which require
//!   "head" scope), but all frames are visible globally so no sensitive data should be
//!   logged without encryption.
//!
//! CONCURRENCY
//! ===========
//! - Emits frames asynchronously via mpsc channel (non-blocking)
//! - FrameStore's writer thread handles persistence in separate thread
//! - Safe for concurrent execution across multiple task lanes
//!
//! TRADE-OFFS
//! ==========
//! 1. **No backpressure on failure**
//!    - CHOSEN: Ignore send failures (`let _ = tx.send(...)`)
//!    - WHY: Prevents logging infrastructure issues from blocking critical operations
//!    - COST: Silent loss of frames if channel disconnected (rare, indicates kernel shutdown)
//!
//! 2. **Required metadata fields**
//!    - CHOSEN: Mandatory `kind` and `scope` fields
//!    - WHY: Ensures frames are filterable via `frames:select` queries
//!    - COST: Callers must structure data rather than free-form logging
//!
//! WHO CAN USE
//! ===========
//! - Any actor (no permission check)
//! - Intended for agent logging, not security-critical operations
//! - All emitted frames are globally visible in the audit trail

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for emitting arbitrary frames into the kernel audit trail.
///
/// WHY: Provides a structured logging API for agents to record custom events,
/// progress markers, or debugging breadcrumbs that persist in frames.db.
pub struct FramesAppend;

impl Default for FramesAppend {
    fn default() -> Self {
        Self::new()
    }
}

impl FramesAppend {
    /// Create a new `FramesAppend` syscall.
    ///
    /// WHY: Zero-state syscall (no configuration needed), so constructor is trivial.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for FramesAppend {
    fn name(&self) -> &'static str {
        "frames:append"
    }

    /// Emit a frame into the audit trail with structured metadata.
    ///
    /// WHY: Allows agents to log custom events that are queryable via `frames:select`.
    /// Unlike ad-hoc logging, frames are persisted to SQLite and indexed by kind/scope.
    ///
    /// ARGUMENTS:
    /// - `kind` (required): Event type for filtering (e.g., "progress", "debug", "metric")
    /// - `scope` (required): Scope/session identifier for isolation (defaults to "main")
    /// - `data` (optional): Arbitrary JSON payload for the event
    ///
    /// RETURNS:
    /// - `Frame::event` with the structured payload
    /// - `Frame::ok` with `{appended: true}` confirmation
    ///
    /// SECURITY NOTE: No permission check - any actor can log. All frames are globally
    /// visible in the audit trail, so do not log sensitive data without encryption.
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // -------------------------------------------------------------------------
        // PHASE 1: Argument Validation
        // WHY: Validate required metadata fields before emitting frame. This ensures
        // frames are queryable via frames:select (which filters on kind/scope).
        // -------------------------------------------------------------------------
        let kind = data
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("log")
            .trim();
        if kind.is_empty() {
            return Err(KernelError::invalid_args("kind is required"));
        }

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        // -------------------------------------------------------------------------
        // PHASE 2: Frame Emission
        // WHY: Emit Frame::event for persistence, then Frame::ok for confirmation.
        // Frames are sent to FrameStore's writer thread for async SQLite writes.
        // -------------------------------------------------------------------------
        let payload = json!({
            "kind": kind,
            "scope": scope,
            "data": data.get("data").cloned().unwrap_or(serde_json::Value::Null),
        });

        // WHY: Ignore send failures to prevent logging infrastructure issues from
        // blocking critical operations. If channel is full or disconnected, we
        // prefer silent loss over cascading syscall failures.
        let _ = tx.send(Frame::event(ctx.call_id, payload)).await;
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"appended": true})))
            .await;
        Ok(())
    }
}
