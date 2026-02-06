//! Task:Read - Retrieve execution logs from SQLite for completed tasks
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall retrieves execution logs (tool calls made by hand agents) from the SQLite
//! history store. Unlike `task:status` (which queries in-memory state), `task:read` queries
//! the persistent `hand_execs` table for auditing and debugging.
//!
//! **Lane assignment: Task lane**
//! - WHY: Queries SQLite store (blocking I/O operation)
//! - Serialization prevents SQLite connection pool exhaustion
//! - Task lane groups task-related operations for consistency
//!
//! **Integration points:**
//! - `history::Store::get_hand_execs()` - Queries hand_execs table by task_id
//! - Frame protocol: Returns execution log with step/tool/success/output
//!
//! **Persistence:**
//! - Reads from SQLite `hand_execs` table (persisted to disk)
//! - Hand agent logs tool calls during execution (not part of task syscalls)
//! - Execution logs survive kernel restarts (unlike in-memory queue state)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Audit trail**: Permanent record of hand agent actions for debugging
//! - **SQLite persistence**: Execution logs survive kernel restarts
//! - **Output truncation**: Tool output limited to 500 chars (prevents huge responses)
//! - **Read-only**: No modification of history (safe to call repeatedly)
//! - **Separate from status**: Status is in-memory (transient), execution log is persistent
//!
//! EXECUTION LOG STRUCTURE
//! =======================
//! Each log entry contains:
//! - `step`: Execution step number (integer sequence)
//! - `tool`: Tool name invoked by hand agent (e.g., "fs:read", "llm:chat")
//! - `success`: Boolean flag (tool call succeeded or failed)
//! - `output`: Tool output truncated to 500 chars (prevents huge responses)
//!
//! USE CASES
//! =========
//! 1. **Debugging**: Inspect tool calls made by hand agent to diagnose failures
//! 2. **Auditing**: Verify hand agent followed instructions correctly
//! 3. **Post-mortem**: Retrieve execution logs after task completion
//! 4. **Testing**: Verify hand agent behavior in integration tests
//!
//! CONCURRENCY
//! ===========
//! - Task lane serialization prevents SQLite connection pool exhaustion
//! - Multiple agents can read different task_ids concurrently (serialized)
//! - No modification of hand_execs table (read-only operation)

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `task:read` syscall.
///
/// WHY: Structured arguments for validation and deserialization.
#[derive(Debug, Deserialize)]
struct TaskReadArgs {
    /// Task ID to retrieve execution logs for.
    ///
    /// WHY: Required field for querying hand_execs table.
    task_id: String,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for retrieving execution logs from SQLite.
///
/// WHY: Encapsulates history store query logic. Delegates to `Store::get_hand_execs()`
/// for actual database access, keeping syscall layer thin.
pub struct TaskRead;

impl TaskRead {
    /// Create a new `TaskRead` syscall.
    ///
    /// WHY: Standard constructor for syscall registration.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskRead {
    fn name(&self) -> &'static str {
        "task:read"
    }

    /// Retrieve execution logs for a task from SQLite history.
    ///
    /// WHY: Enables agents to inspect tool calls made by hand agents during task
    /// execution. Used for debugging failures, auditing behavior, or post-mortem analysis.
    ///
    /// USE CASE: Head agent notices task failed (via `task:status`), calls `task:read`
    /// to retrieve execution logs and diagnose what went wrong.
    ///
    /// ARGUMENTS:
    /// - `task_id` (required): Unique task identifier from enqueue
    ///
    /// RETURNS:
    /// - `Frame::ok` with execution log:
    ///   - `id`: Task ID (echoed back)
    ///   - `status`: Always "completed" (historical logs only)
    ///   - `execution_log`: Array of {step, tool, success, output} entries
    ///   - `steps`: Total number of tool calls logged
    /// - `E_NOT_FOUND` if task_id has no execution logs in history
    /// - `E_INVALID_ARGS` if task_id is missing or empty
    /// - `E_INTERNAL` if kernel or store not initialized
    /// - `E_IO` if SQLite query fails
    ///
    /// OUTPUT TRUNCATION:
    /// - Tool output truncated to 500 chars per entry (prevents huge responses)
    /// - Step/tool/success always included (not truncated)
    ///
    /// PERSISTENCE:
    /// - Reads from SQLite `hand_execs` table (persisted to disk)
    /// - Execution logs survive kernel restarts (unlike in-memory queue state)
    /// - Hand agent logs tool calls via `Store::log_hand_exec()` during execution
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // WHY: Early cancellation check prevents wasted work on cancelled operations.
        ctx.check_cancelled()?;

        // WHY: Kernel::get() retrieves global kernel instance for Store access.
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // WHY: Store::get() retrieves SQLite store for history queries. Returns E_INTERNAL
        // if store not attached (should never happen in production).
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        // WHY: Deserialize and validate arguments. Structured args provide clear error
        // messages for malformed requests.
        let args: TaskReadArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let task_id = args.task_id.trim();
        if task_id.is_empty() {
            return Err(KernelError::invalid_args("task_id is required"));
        }

        // WHY: Store::get_hand_execs() queries hand_execs table for task_id. Returns
        // empty Vec if no logs found. Blocking I/O operation (SQLite query).
        let execs = store
            .get_hand_execs(task_id)
            .map_err(|e| KernelError::io(format!("query error: {e}")))?;

        // WHY: E_NOT_FOUND if task has no execution logs. This distinguishes "task never
        // executed" from "task executed but logs empty" (though latter is rare).
        if execs.is_empty() {
            return Err(KernelError::not_found(format!("task not found: {}", task_id)));
        }

        // WHY: Convert HandExec records to JSON log entries. Truncate output to 500 chars
        // to prevent huge responses (full output stored in SQLite, but not returned here).
        let logs: Vec<_> = execs
            .iter()
            .map(|e| {
                // WHY: chars().take(500) truncates output at character boundary (not byte
                // boundary), preventing invalid UTF-8 truncation.
                let output: String = e.output.chars().take(500).collect();
                json!({
                    "step": e.step,
                    "tool": e.tool,
                    "success": e.success,
                    "output": output
                })
            })
            .collect();

        // WHY: Return execution log with metadata. status="completed" because this syscall
        // only returns historical logs (not in-progress execution state).
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "id": task_id,
                    "status": "completed",
                    "execution_log": logs,
                    "steps": logs.len()
                }),
            ))
            .await;

        Ok(())
    }
}
