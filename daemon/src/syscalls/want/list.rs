//! Want:List - Retrieve wants from queue
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Queries the persistent want queue and returns a priority-sorted list of wants.
//! Used by mind agents to review accumulated goals before deciding which to promote
//! to executable needs.
//!
//! **Integration:** Reads from SQLite via `Store::list_wants(limit)`. The store
//! implementation handles priority ordering (urgent first, then normal) and limits
//! result count to prevent memory exhaustion.
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{"wants": [...], "count": N}`
//! - Returns `E_INVALID_ARGS` if limit is malformed
//! - Returns `E_INTERNAL` if kernel or store is not initialized
//! - Returns `E_IO` if database query fails
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Limited by default**: Defaults to 20 results to prevent overwhelming UI/LLM context
//! - **Clamped maximum**: Hard limit of 100 to prevent memory exhaustion
//! - **Priority-sorted**: Store returns urgent wants first for natural prioritization
//! - **Read-only operation**: No mutation, safe for concurrent access
//!
//! USE CASES
//! =========
//! - **Reflection review**: Mind agents read accumulated wants before room sessions
//! - **Promotion decisions**: List wants to choose which to promote to needs
//! - **Queue monitoring**: Check want backlog size and priorities

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `want:list` syscall.
///
/// WHY: Single optional field keeps interface minimal while allowing pagination.
#[derive(Debug, Deserialize)]
struct WantListArgs {
    /// Maximum number of wants to return (default: 20, max: 100).
    ///
    /// WHY: Limit prevents memory exhaustion from large result sets.
    #[serde(default)]
    limit: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for listing wants from the queue.
///
/// WHY: Stateless unit struct since listing requires no configuration.
pub struct WantList;

impl WantList {
    /// Create a new `WantList` syscall.
    ///
    /// WHY: Standard constructor for consistency with other syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for WantList {
    fn name(&self) -> &'static str {
        "want:list"
    }

    /// List wants from the persistent queue.
    ///
    /// WHY: Enables mind agents to review accumulated goals before deciding which
    /// to promote to executable needs. Priority-sorted results (urgent first) support
    /// natural decision-making flow.
    ///
    /// USE CASE: Invoked by mind agents before room sessions to load context about
    /// pending goals, or after sessions to choose which wants to promote.
    ///
    /// PERFORMANCE: Limited to 100 results maximum to prevent memory exhaustion.
    /// For large want queues, callers should implement pagination or filtering
    /// (future enhancement: add offset/priority filter parameters).
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"wants": [...], "count": N}`
    /// - `E_INVALID_ARGS` if limit is malformed
    /// - `E_INTERNAL` if kernel or store is not initialized
    /// - `E_IO` if database query fails
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

        let args: WantListArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Default to 20 results (reasonable for UI/LLM context window).
        // Clamp to 100 maximum to prevent memory exhaustion from unbounded queries.
        // Minimum of 1 prevents zero-result confusion (caller should omit limit if
        // they want default, not pass 0).
        let limit = args.limit.unwrap_or(20).clamp(1, 100);

        // WHY: Store::list_wants returns priority-sorted results (urgent first)
        let wants = store
            .list_wants(limit)
            .map_err(|e| KernelError::io(format!("failed to list wants: {e}")))?;

        // WHY: Transform store representation to JSON for frame protocol.
        // Include all fields (id, want, context, priority, source) so callers
        // have complete information for promotion decisions.
        let items: Vec<_> = wants
            .into_iter()
            .map(|w| {
                json!({
                    "id": w.id,
                    "want": w.want,
                    "context": w.context,
                    "priority": w.priority,
                    "source": w.source
                })
            })
            .collect();

        // WHY: Return both wants array and count for caller convenience
        // (avoids need to compute items.len() on client side)
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"wants": items, "count": items.len()}),
            ))
            .await;

        Ok(())
    }
}
