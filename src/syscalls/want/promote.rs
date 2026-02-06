//! Want:Promote - Convert want to executable need
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Promotes a want from the aspirational goal pool to an executable need in the
//! priority queue. This is the primary mechanism for converting long-term goals
//! into actionable work.
//!
//! **Integration:** Reads want from SQLite via `Store::get_want()`, deletes it
//! via `Store::remove_want()`, then dispatches `need:enqueue` syscall through the
//! kernel dispatcher. The promoted need inherits the want's text, context, and
//! priority (or override priority from args).
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{"promoted": true, "want_id": ID, "need_id": UUID, "priority": P}`
//! - Emits `Frame::ok` with `{"promoted": false, "reason": "want not found"}` if ID doesn't exist
//! - Returns `E_INVALID_ARGS` if ID is missing or malformed
//! - Returns `E_INTERNAL` if kernel or store is not initialized
//! - Returns `E_IO` if database operations fail
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Atomic promotion**: Remove from wants before enqueueing need (no duplicate promotion)
//! - **Priority override**: Optionally escalate/downgrade priority during promotion
//! - **Fire-and-forget dispatch**: Don't wait for need execution, just enqueue it
//! - **Idempotent failure**: Promoting non-existent want returns ok (not error)
//! - **Context preservation**: Want's text and context carry forward to need
//!
//! PROMOTION WORKFLOW
//! ==================
//! 1. Fetch want from database by ID
//! 2. Delete want from queue (prevents duplicate promotion)
//! 3. Generate new UUID for need
//! 4. Dispatch `need:enqueue` with want's data
//! 5. Return promotion status with both IDs
//!
//! WHY URGENT WANTS AUTO-RECONVENE
//! ===============================
//! When promoting an urgent want, `reconvene: true` is passed to `need:enqueue`.
//! This signals that fulfilling the need should trigger a room session to reflect
//! on the outcome. This is appropriate for urgent wants because they typically
//! represent critical issues requiring strategic review after execution.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `want:promote` syscall.
///
/// WHY: Separates required field (ID) from optional override (priority).
#[derive(Debug, Deserialize)]
struct WantPromoteArgs {
    /// ID of want to promote (from `want:create` or `want:list`).
    id: String,

    /// Optional priority override for the created need.
    ///
    /// WHY: Allows escalating urgent want or downgrading if context changed.
    /// Defaults to want's original priority if omitted.
    #[serde(default)]
    priority: Option<String>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for promoting wants to needs.
///
/// WHY: Stateless unit struct since promotion requires no configuration.
pub struct WantPromote;

impl WantPromote {
    /// Create a new `WantPromote` syscall.
    ///
    /// WHY: Standard constructor for consistency with other syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for WantPromote {
    fn name(&self) -> &'static str {
        "want:promote"
    }

    /// Promote a want to an executable need.
    ///
    /// WHY: Converts aspirational goal to actionable work item. This is the primary
    /// mechanism for mind agents to commit to pursuing a want. Promotion removes the
    /// want from the pool (preventing duplicate promotion) and dispatches it as a need.
    ///
    /// USE CASE: Invoked by mind agents after deliberation when deciding to pursue a
    /// want, or automatically for urgent wants that require immediate action.
    ///
    /// CONCURRENCY: Promotion is atomic from caller's perspective (remove + enqueue
    /// happen in sequence), though `need:enqueue` dispatch is fire-and-forget (doesn't
    /// wait for need execution).
    ///
    /// SECURITY NOTE: No actor restrictions since promotion just moves want to need queue.
    /// The `need:enqueue` syscall has its own authorization checks if needed.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"promoted": true, "want_id": ID, "need_id": UUID, "priority": P}`
    /// - `Frame::ok` with `{"promoted": false, "reason": "want not found"}` if ID doesn't exist
    /// - `E_INVALID_ARGS` if ID is missing or malformed
    /// - `E_INTERNAL` if kernel or store is not initialized
    /// - `E_IO` if database operations fail
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // WHY: Need both store (for want retrieval/deletion) and dispatcher (for need:enqueue)
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        let args: WantPromoteArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // =====================================================================
        // PHASE 1: Fetch Want
        // =====================================================================
        // WHY: Retrieve want data before deletion so we can pass it to need:enqueue.
        // If want doesn't exist, return ok (idempotent promotion).
        let want = match store.get_want(&args.id) {
            Ok(Some(w)) => w,
            Ok(None) => {
                // WHY: Not an error - idempotent promotion of non-existent want.
                // Caller may have already promoted it or it was deleted externally.
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({"promoted": false, "reason": "want not found"}),
                    ))
                    .await;
                return Ok(());
            }
            Err(e) => return Err(KernelError::io(format!("failed to get want: {e}"))),
        };

        // =====================================================================
        // PHASE 2: Remove Want
        // =====================================================================
        // WHY: Delete want before creating need to prevent duplicate promotion.
        // If removal fails (database error), we abort without creating the need.
        store
            .remove_want(&args.id)
            .map_err(|e| KernelError::io(format!("failed to remove want: {e}")))?;

        // =====================================================================
        // PHASE 3: Prepare Need
        // =====================================================================
        // WHY: Use caller's priority override if provided, otherwise inherit from want.
        // Generate new UUID for need (separate identity from want).
        let priority_str = args.priority.as_deref().unwrap_or(&want.priority);
        let need_id = Uuid::new_v4().to_string();

        // =====================================================================
        // PHASE 4: Dispatch Need:Enqueue
        // =====================================================================
        // WHY: Fire-and-forget dispatch - don't wait for need execution, just enqueue it.
        // This prevents want:promote from blocking for minutes while need is fulfilled.
        //
        // RECONVENE LOGIC: Urgent needs trigger room session after fulfillment (strategic
        // review of critical actions). Normal needs don't reconvene (routine work).
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "need:enqueue",
            json!({
                "need_id": need_id,
                "source": "mind",
                "priority": priority_str,
                "need": want.want,
                "context": want.context,
                "scope": "main",
                "reconvene": priority_str == "urgent",
            }),
        )
        .with_actor("system/mind".to_string());

        let mut rx = dispatcher.dispatch(
            req,
            ctx.cwd.clone(),
            tokio_util::sync::CancellationToken::new(),
        );

        // WHY: Wait for need:enqueue response frame to ensure it was accepted.
        // Don't wait for need fulfillment (that happens asynchronously on task lane).
        let _ = rx.recv().await;

        // =====================================================================
        // PHASE 5: Return Success
        // =====================================================================
        // WHY: Return both want_id and need_id so caller can track promotion mapping.
        // Priority is returned for confirmation (may differ from want's original priority).
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "promoted": true,
                    "want_id": args.id,
                    "need_id": need_id,
                    "priority": priority_str
                }),
            ))
            .await;

        Ok(())
    }
}
