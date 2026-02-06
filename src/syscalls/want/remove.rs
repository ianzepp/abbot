//! Want:Remove - Delete want from queue
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Removes a want from the persistent queue by ID. Used to clean up stale wants,
//! cancel goals that are no longer relevant, or explicitly delete wants before
//! promoting them (though `want:promote` handles deletion automatically).
//!
//! **Integration:** Deletes from SQLite via `Store::remove_want(id)`. Returns
//! success status even if want doesn't exist (idempotent deletion).
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{"removed": true}` if want was found and deleted
//! - Emits `Frame::ok` with `{"removed": false, "reason": "not found"}` if ID doesn't exist
//! - Returns `E_INVALID_ARGS` if ID is missing or malformed
//! - Returns `E_INTERNAL` if kernel or store is not initialized
//! - Returns `E_IO` if database deletion fails
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Idempotent deletion**: Success even if want doesn't exist (prevents retry errors)
//! - **Explicit feedback**: Returns whether deletion actually occurred
//! - **No cascading deletes**: Only deletes the want, doesn't affect promoted needs
//! - **ID-based operation**: Requires exact ID from prior `want:list` or `want:create`
//!
//! USE CASES
//! =========
//! - **Stale goal cleanup**: Remove wants that are no longer relevant
//! - **Explicit cancellation**: Delete want after manually addressing it
//! - **Error recovery**: Clean up duplicate or malformed wants

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `want:remove` syscall.
///
/// WHY: Single required field - need ID to identify which want to delete.
#[derive(Debug, Deserialize)]
struct WantRemoveArgs {
    /// ID of want to remove (from `want:create` or `want:list`).
    id: String,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for removing wants from the queue.
///
/// WHY: Stateless unit struct since removal requires no configuration.
pub struct WantRemove;

impl WantRemove {
    /// Create a new `WantRemove` syscall.
    ///
    /// WHY: Standard constructor for consistency with other syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for WantRemove {
    fn name(&self) -> &'static str {
        "want:remove"
    }

    /// Remove a want from the persistent queue.
    ///
    /// WHY: Enables cleanup of stale or completed wants. Idempotent deletion
    /// (succeeds even if want doesn't exist) prevents retry errors when callers
    /// aren't sure whether want still exists.
    ///
    /// USE CASE: Invoked by mind agents to clean up wants that are no longer
    /// relevant, or by maintenance scripts to prune old entries.
    ///
    /// NOTE: Deleting a want doesn't affect needs that were promoted from it.
    /// Once promoted, the need exists independently of the original want.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"removed": true}` if want was found and deleted
    /// - `Frame::ok` with `{"removed": false, "reason": "not found"}` if ID doesn't exist
    /// - `E_INVALID_ARGS` if ID is missing or malformed
    /// - `E_INTERNAL` if kernel or store is not initialized
    /// - `E_IO` if database deletion fails
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // WHY: Store is required for database access
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        let args: WantRemoveArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Store::remove_want returns Ok(true) if deleted, Ok(false) if not found.
        // Both are success cases (idempotent deletion), but we return different status
        // so caller knows whether deletion actually occurred.
        match store.remove_want(&args.id) {
            Ok(true) => {
                let _ = tx
                    .send(Frame::ok(ctx.call_id, json!({"removed": true})))
                    .await;
                Ok(())
            }
            Ok(false) => {
                // WHY: Not an error - successful idempotent deletion of non-existent want.
                // Return Frame::ok but indicate nothing was removed so caller can log if needed.
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({"removed": false, "reason": "not found"}),
                    ))
                    .await;
                Ok(())
            }
            Err(e) => {
                // WHY: Database error is actual failure (not idempotent case)
                Err(KernelError::io(format!("failed to remove want: {e}")))
            }
        }
    }
}
