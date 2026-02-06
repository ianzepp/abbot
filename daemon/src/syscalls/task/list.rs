//! Task:List - List tasks with status filtering (placeholder implementation)
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall is a **placeholder** for future task listing functionality. Currently returns
//! an empty list regardless of arguments. Intended for querying all tasks with optional status
//! filtering (queued/running/done) and pagination.
//!
//! **Lane assignment: Task lane**
//! - WHY: Would query TaskKernel state (if implemented)
//! - Serialization ensures consistent snapshots of task state
//! - Task lane groups task-related operations for consistency
//!
//! **Current status: PLACEHOLDER**
//! - Always returns empty task list
//! - Arguments parsed and validated but not used
//! - Intended for future implementation when task listing is needed
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Future-proofing**: Defines API contract for when listing is implemented
//! - **Status filtering**: Allows querying only queued/running/done tasks
//! - **Pagination**: Limit parameter prevents unbounded responses
//! - **Placeholder pattern**: Returns empty results rather than error (forwards-compatible)
//!
//! INTENDED BEHAVIOR (when implemented)
//! ====================================
//! Would query `TaskKernel::active` map and return filtered list of tasks:
//! - Filter by status: "queued", "running", "done", or "all"
//! - Limit results to prevent unbounded responses (default 50, max 100)
//! - Return task summaries (id, status, scope, prompt) not full details
//!
//! CONCURRENCY
//! ===========
//! - Task lane serialization would ensure consistent task list snapshots
//! - Multiple agents could list concurrently (serialized reads)
//! - No modification of task state (read-only operation)

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `task:list` syscall.
///
/// WHY: Structured arguments for status filtering and pagination.
#[derive(Debug, Deserialize)]
struct TaskListArgs {
    /// Filter tasks by status (queued/running/done/all).
    ///
    /// WHY: Allows querying specific task states (e.g., only running tasks).
    /// Defaults to "all" if not specified.
    #[serde(default)]
    status: Option<String>,

    /// Maximum number of tasks to return.
    ///
    /// WHY: Prevents unbounded responses when many tasks exist.
    /// Defaults to 50, clamped to [1, 100] range.
    #[serde(default)]
    limit: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for listing tasks with status filtering (placeholder implementation).
///
/// WHY: Defines API contract for future task listing functionality. Currently
/// returns empty list, but arguments are parsed and validated for forwards compatibility.
pub struct TaskList;

impl TaskList {
    /// Create a new `TaskList` syscall.
    ///
    /// WHY: Standard constructor for syscall registration.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskList {
    fn name(&self) -> &'static str {
        "task:list"
    }

    /// List tasks with optional status filtering (placeholder implementation).
    ///
    /// WHY: Placeholder for future task listing functionality. Defines API contract
    /// but currently returns empty list. Enables client code to call `task:list`
    /// without errors, simplifying migration when implementation is added.
    ///
    /// USE CASE (when implemented): Dashboard showing all running tasks, or cleanup
    /// script finding stuck tasks that need manual intervention.
    ///
    /// ARGUMENTS:
    /// - `status` (optional): Filter by status ("queued", "running", "done", "all")
    /// - `limit` (optional): Maximum results (default 50, max 100)
    ///
    /// RETURNS:
    /// - `Frame::ok` with empty task list:
    ///   - `tasks`: Always empty array (placeholder)
    ///   - `count`: Always 0 (placeholder)
    ///   - `status_filter`: Echoed back from arguments
    /// - `E_INVALID_ARGS` if arguments are malformed
    ///
    /// PLACEHOLDER STATUS:
    /// - Arguments parsed and validated but not used
    /// - Always returns empty list regardless of filters
    /// - No queries to TaskKernel (would be implemented here)
    ///
    /// INTENDED IMPLEMENTATION:
    /// - Query `TaskKernel::active` map for all tasks
    /// - Filter by status (match TaskStatus enum)
    /// - Sort by created_at descending (newest first)
    /// - Apply limit for pagination
    /// - Return task summaries (not full details)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // WHY: Early cancellation check prevents wasted work on cancelled operations.
        ctx.check_cancelled()?;

        // WHY: Deserialize and validate arguments even though not used (validates API contract).
        let args: TaskListArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Extract status filter and limit with sensible defaults. Clamp limit to [1, 100]
        // to prevent unbounded responses (when implemented).
        let status_filter = args.status.as_deref().unwrap_or("all");
        let limit = args.limit.unwrap_or(50).clamp(1, 100);

        // WHY: Placeholder implementation - always returns empty list. When implemented,
        // this would query TaskKernel::active map and filter by status.
        let tasks: Vec<serde_json::Value> = Vec::new();
        let _ = limit; // Silence unused variable warning
        let _ = status_filter; // Silence unused variable warning

        // WHY: Return empty list with status_filter echoed back. Forwards-compatible with
        // future implementation (same response structure).
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "tasks": tasks,
                    "count": tasks.len(),
                    "status_filter": status_filter
                }),
            ))
            .await;

        Ok(())
    }
}
