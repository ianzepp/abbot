//! State:Query - Introspect kernel runtime state via SQLite Store queries
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides mode-based introspection of kernel runtime state stored in
//! the SQLite-backed history Store (`<workspace>/.abbot/store.db`). It supports:
//!
//! **Active query modes:**
//! - `wants`: List aspirational goals in the pool (Mind/Room agent work items)
//! - `logs`: Retrieve hand execution history for a specific task_id
//! - `stats`: Aggregate statistics (wants count, metadata)
//!
//! **Deprecated modes (return helpful errors):**
//! - `messages`: Removed (Frame-based communication replaced bus/store.db)
//! - `needs`: Removed (wants pool is primary state)
//! - `tasks`/`goals`: Removed (external task manager)
//!
//! **Storage architecture:**
//! - Wants: `wants` table (id, want, context, priority, source, created_at)
//! - Logs: `hand_exec` table (task_id, step, tool, args, output, success, timestamp)
//! - Stats: Computed via COUNT(*) aggregation (no cached values)
//!
//! **Query patterns:**
//! - Wants: Sorted by priority (urgent > high > normal > low), then creation time
//! - Logs: Filtered by task_id, sorted by step (chronological execution order)
//! - Limit: Clamped to 1-100 rows (default 20) to prevent runaway queries
//!
//! **Integration points:**
//! - `Kernel::get()` for kernel singleton access
//! - `Kernel::store()` for SQLite Store handle (returns None if Store not attached)
//! - `Store::list_wants()`, `Store::get_hand_execs()`, `Store::count_wants()` for data access
//! - `SyscallContext` for cancellation checking
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with JSON result: `{wants: [...], count: N}` or `{logs: [...], count: N}`
//! - Returns `E_INTERNAL` if kernel or Store not initialized
//! - Returns `E_INVALID_ARGS` for unknown/deprecated modes or missing required args
//! - Returns `E_IO` for SQLite query errors
//!
//! SECURITY MODEL
//! ==============
//! This syscall is **read-only introspection** with no mutation guard:
//!
//! 1. **No Mutation Permission Required**
//!    - WHY: Read-only queries are inherently safe (no side effects)
//!    - IMPLICATION: All agent types ("head", "hand", "room") may query state
//!    - RATIONALE: Introspection enables debugging, adaptive behavior, progress monitoring
//!
//! 2. **Global State Access (No Actor Scoping)**
//!    - WHY: All agents share the same wants pool and can see all execution logs
//!    - HOW: No actor-based WHERE clauses in SQL queries
//!    - TRADE-OFF: No privacy isolation (agent A can see agent B's logs)
//!    - ACCEPTABLE: Single-user workspace model, no multi-tenant security requirements
//!    - IMPLICATION: If multi-user support added, this syscall needs actor filtering
//!
//! 3. **Output Limiting**
//!    - WHY: Prevents memory exhaustion from unbounded result sets
//!    - HOW: `limit` parameter clamped to 1-100 (default 20) at line 92
//!    - ATTACK PREVENTED: Runaway queries returning millions of rows
//!    - TRADE-OFF: Large datasets require multiple calls (no pagination support yet)
//!
//! 4. **Output Truncation**
//!    - WHY: Prevents oversized JSON responses from verbose tool outputs
//!    - HOW: `hand_exec.output` truncated to 200 chars (line 129)
//!    - RATIONALE: Full output available via separate log file if needed
//!    - IMPLICATION: Introspection sees summaries, not complete execution traces
//!
//! 5. **No Write/Delete Operations**
//!    - WHY: Introspection should not mutate audit trails
//!    - HOW: Only SELECT queries issued (no INSERT/UPDATE/DELETE)
//!    - IMPLICATION: State cleanup requires separate syscalls or manual DB access
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Mode-based dispatch**: Single syscall with extensible mode parameter
//! - **Read-only queries**: Introspection never mutates state (audit preservation)
//! - **Output limiting**: Bounded result sets prevent resource exhaustion
//! - **Graceful deprecation**: Removed modes return helpful errors (not panics)
//! - **Priority-aware sorting**: Wants ordered by priority then creation time
//! - **Chronological logs**: Execution history sorted by step (reproducible order)
//!
//! WHY MODE-BASED DESIGN:
//! - Single syscall reduces kernel registration overhead
//! - Easier to add new introspection modes (just add match arm)
//! - Consistent query interface (all modes use `limit`, similar result format)
//! - Alternative rejected: Separate `state:wants`, `state:logs` syscalls
//!   would clutter registry and require duplicate boilerplate
//!
//! WHY PRIORITY SORTING FOR WANTS:
//! - Ensures agents see highest-priority work first (urgent > high > normal > low)
//! - FIFO within same priority (created_at ASC ensures fairness)
//! - Matches intuitive expectation for work queue semantics
//!
//! CONCURRENCY
//! ===========
//! - SQLite Store uses `Mutex<Connection>` for thread-safety (single writer)
//! - Queries acquire lock briefly (no long-running transactions)
//! - Safe for concurrent execution across multiple task lanes
//! - No deadlock risk: Queries complete in microseconds-milliseconds
//! - Blocking: Other queries wait for lock (acceptable for infrequent introspection)
//!
//! PERFORMANCE
//! ===========
//! - **Wants query**: `SELECT ... ORDER BY priority, created_at LIMIT N`
//!   - No index on priority (acceptable: wants pool typically <1000 rows)
//!   - Sub-millisecond for typical workloads (<100 wants)
//! - **Logs query**: `SELECT ... WHERE task_id = ? ORDER BY step ASC`
//!   - Indexed via `idx_hand_exec_task` (fast lookup by task_id)
//!   - Sub-millisecond for typical tasks (<100 steps)
//! - **Stats query**: `SELECT COUNT(*) FROM wants`
//!   - No cached value (always reflects current state)
//!   - Microsecond-level response time
//! - **Output truncation**: 200-char limit on hand_exec.output prevents large JSON payloads
//! - **Bottleneck**: SQLite lock contention under heavy concurrent introspection
//!
//! TRADE-OFFS
//! ==========
//! 1. **Global Access vs. Actor Scoping**
//!    - CHOSEN: Global access (all agents see same state)
//!    - REJECTED: Actor-based filtering (WHERE actor = ctx.actor)
//!    - WHY: Single-user workspace model, no multi-tenant security
//!    - IMPLICATION: Agents can see each other's logs (acceptable for debugging)
//!
//! 2. **Mode-Based Dispatch vs. Separate Syscalls**
//!    - CHOSEN: Single `state:query` with `mode` parameter
//!    - REJECTED: `state:wants`, `state:logs`, `state:stats` as separate syscalls
//!    - WHY: Reduces syscall registration boilerplate, easier to extend
//!    - IMPLICATION: Not type-safe at compile time (must validate mode at runtime)
//!
//! 3. **No Pagination**
//!    - CHOSEN: Clamp limit to 1-100, no offset/cursor support
//!    - WHY: Simple implementation, sufficient for typical debugging
//!    - IMPLICATION: Cannot query more than 100 rows per call
//!
//! 4. **No Caching**
//!    - CHOSEN: Query SQLite directly on every call
//!    - REJECTED: In-memory cache with invalidation
//!    - WHY: Consistency guarantees (always see latest state), simpler implementation
//!    - IMPLICATION: Slower than cached reads, but acceptable for infrequent introspection
//!
//! 5. **Output Truncation vs. Pagination**
//!    - CHOSEN: Truncate hand_exec.output to 200 chars
//!    - REJECTED: Return full output, paginate results
//!    - WHY: Introspection shows summaries, full logs available via separate mechanism
//!    - IMPLICATION: Cannot see complete tool outputs via this syscall
//!
//! ERROR HANDLING
//! ==============
//! - `E_INTERNAL`: Kernel or Store not initialized (should never happen in production)
//! - `E_INVALID_ARGS`: Unknown mode, deprecated mode, or missing required args (task_id)
//! - `E_IO`: SQLite query errors (database corruption, disk full, etc.)
//! - `E_CANCELLED`: Parent context cancelled during query (rare: queries are fast)

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `state:query` syscall.
///
/// WHY: Mode-based dispatch with optional parameters for flexibility.
/// Different modes require different arguments (e.g., `logs` needs `task_id`).
#[derive(Debug, Deserialize)]
struct StateQueryArgs {
    /// Query mode: "wants", "logs", "stats" (or deprecated modes for error messages).
    ///
    /// WHY: Extensible design - new introspection modes can be added without new syscalls.
    mode: String,

    /// Scope filter (reserved for future multi-scope support).
    ///
    /// WHY: Placeholder for future actor-scoped queries (not yet implemented).
    /// Currently ignored (all queries are global scope).
    #[serde(default)]
    #[allow(dead_code)]
    scope: Option<String>,

    /// Task ID for logs mode (required when mode = "logs").
    ///
    /// WHY: Execution logs are scoped to specific tasks, not global.
    /// SECURITY: No validation that caller owns this task_id (global access).
    #[serde(default)]
    task_id: Option<String>,

    /// Maximum number of results to return (default: 20, clamped: 1-100).
    ///
    /// WHY: Prevents runaway queries from returning unbounded result sets.
    /// TRADE-OFF: No pagination support, so max 100 rows per call.
    #[serde(default)]
    limit: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for querying kernel runtime state via SQLite Store.
///
/// WHY: Stateless syscall - no internal state beyond the Syscall trait implementation.
/// All state is fetched from Kernel::get() and Store on each invocation.
pub struct StateQuery;

impl StateQuery {
    /// Create a new `StateQuery` syscall.
    ///
    /// WHY: Zero-sized struct - no configuration needed for read-only operation.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for StateQuery {
    fn name(&self) -> &'static str {
        "state:query"
    }

    /// Query kernel runtime state with mode-based dispatch.
    ///
    /// WHY: Enables introspection of kernel state (wants pool, execution logs, statistics)
    /// for debugging, progress monitoring, and adaptive agent behavior. Read-only operation
    /// safe for all agent types without mutation permission.
    ///
    /// USE CASE: Invoked by agents and debugging tools to query:
    /// - Wants pool: See pending work items for Mind/Room agents
    /// - Execution logs: Audit trail of hand tool calls for specific task
    /// - Statistics: Aggregate metrics for monitoring kernel health
    ///
    /// QUERY MODES:
    /// - `wants`: List aspirational goals in pool (sorted by priority, then creation time)
    /// - `logs`: Retrieve hand execution history for specific task_id (chronological steps)
    /// - `stats`: Aggregate statistics (wants count, metadata)
    ///
    /// DEPRECATED MODES (return helpful errors):
    /// - `messages`: Removed (Frame-based communication replaced bus)
    /// - `needs`: Removed (wants pool is primary state)
    /// - `tasks`/`goals`: Removed (external task manager)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{wants: [...], count: N}` for wants mode
    /// - `Frame::ok` with `{logs: [...], count: N}` for logs mode
    /// - `Frame::ok` with `{wants_pool: N, note: "..."}` for stats mode
    /// - `E_INTERNAL` if kernel or Store not initialized
    /// - `E_INVALID_ARGS` if mode unknown/deprecated or task_id missing for logs
    /// - `E_IO` for SQLite query errors
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Kernel & Store Access
        // =====================================================================
        // WHY: Check cancellation first to avoid wasted work. Then verify kernel
        // and Store are initialized (defensive: should always be true in production).
        ctx.check_cancelled()?;

        // WHY: Kernel singleton access. Returns None if kernel not started
        // (should never happen: syscalls only dispatched after kernel starts).
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // WHY: Store handle for SQLite queries. Returns None if Store not attached
        // (happens if kernel started without --store flag, rare in production).
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        // =====================================================================
        // PHASE 2: Argument Parsing & Validation
        // =====================================================================
        // WHY: Parse JSON arguments into strongly-typed struct. Fail fast on
        // invalid JSON schema (missing mode, wrong types, etc.).
        let args: StateQueryArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Clamp limit to 1-100 (default 20) to prevent runaway queries.
        // SECURITY: Prevents memory exhaustion from unbounded result sets.
        // TRADE-OFF: No pagination support, so max 100 rows per call.
        let limit = args.limit.unwrap_or(20).clamp(1, 100);

        // =====================================================================
        // PHASE 3: Mode-Based Query Dispatch
        // =====================================================================
        // WHY: Match on mode string to dispatch to appropriate Store method.
        // Deprecated modes return helpful errors explaining why they were removed.
        let result = match args.mode.as_str() {
            // WHY: Deprecated mode - message history removed with bus/store.db redesign.
            // Return error with explanation instead of silently failing or panicking.
            "messages" => {
                return Err(KernelError::invalid_args(
                    "introspect mode 'messages' removed (bus/store.db message history deprecated)",
                ));
            }

            // WHY: Wants mode - list aspirational goals in pool (Mind/Room work items).
            // Sorted by priority then creation time (FIFO). Now backed by EMS.
            "wants" => {
                let Some(ems) = k.ems() else {
                    return Err(KernelError::internal("EMS not attached"));
                };
                let wants = {
                    let ems = ems.lock().unwrap();
                    ems.select(
                        "wants",
                        Some(&json!({"status": "pending"})),
                        None,
                        Some(&json!(["priority ASC", "created_at ASC"])),
                        Some(limit),
                        None,
                    )
                    .map_err(|e| KernelError::io(format!("query error: {e}")))?
                };

                let out: Vec<_> = wants
                    .iter()
                    .map(|w| {
                        let id = w.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        json!({
                            "id": &id[..8.min(id.len())],
                            "want": w.get("want").and_then(|v| v.as_str()).unwrap_or(""),
                            "priority": w.get("priority").and_then(|v| v.as_str()).unwrap_or("normal"),
                            "source": w.get("source").and_then(|v| v.as_str()).unwrap_or("mind"),
                        })
                    })
                    .collect();
                json!({"wants": out, "count": out.len()})
            }

            // WHY: Logs mode - retrieve hand execution history for specific task.
            // Sorted by step (chronological execution order), truncated to 200 chars.
            "logs" => {
                // WHY: task_id is required for logs mode (execution logs are task-scoped).
                // Return error if missing instead of returning all logs (would be huge).
                let task_id = args
                    .task_id
                    .as_deref()
                    .ok_or_else(|| KernelError::invalid_args("task_id required for logs mode"))?;

                // WHY: Store::get_hand_execs() queries SQLite: SELECT ... WHERE task_id = ? ORDER BY step ASC
                // Returns Vec<HandExec> with step, tool, args, output, success, timestamp.
                let execs = store
                    .get_hand_execs(task_id)
                    .map_err(|e| KernelError::io(format!("query error: {e}")))?;

                // WHY: Transform HandExec structs to JSON, truncating output to 200 chars.
                // TRADE-OFF: Full output available in Store if needed, introspection shows summary.
                // RATIONALE: Tool outputs can be megabytes (e.g., git log), truncation prevents huge responses.
                let out: Vec<_> = execs
                    .iter()
                    .map(|e| {
                        let output: String = e.output.chars().take(200).collect(); // WHY: Truncate to 200 chars
                        json!({
                            "step": e.step,
                            "tool": e.tool,
                            "success": e.success,
                            "output": output
                        })
                    })
                    .collect();
                json!({"logs": out, "count": out.len()})
            }

            // WHY: Stats mode - aggregate statistics for monitoring kernel health.
            // Wants count now from EMS.
            "stats" => {
                let wants_pool = if let Some(ems) = k.ems() {
                    let ems = ems.lock().unwrap();
                    ems.select("wants", Some(&json!({"status": "pending"})), None, None, None, None)
                        .map(|rows| rows.len())
                        .unwrap_or(0)
                } else { 0 };
                json!({
                    "wants_pool": wants_pool,
                    "note": "recent message stats removed (store.db message history deprecated)"
                })
            }

            // WHY: Deprecated modes - needs tracking removed with wants pool redesign.
            // Return error with explanation instead of silently failing.
            "needs" => {
                return Err(KernelError::invalid_args(
                    "introspect mode 'needs' removed (store.db message history deprecated)",
                ));
            }

            // WHY: Deprecated modes - task tracking removed with external task manager.
            // Return error with explanation instead of silently failing.
            "tasks" | "goals" => {
                return Err(KernelError::invalid_args(
                    "introspect mode 'tasks' removed (store.db message history deprecated)",
                ));
            }

            // WHY: Unknown mode - return error with mode name for debugging.
            // Prevents typos from silently failing (e.g., "want" instead of "wants").
            _ => {
                return Err(KernelError::invalid_args(format!(
                    "unknown introspect mode: {}",
                    args.mode
                )));
            }
        };

        // =====================================================================
        // PHASE 4: Response Emission
        // =====================================================================
        // WHY: Send Frame::ok with query results as JSON. Ignore send errors
        // (receiver dropped means caller no longer listening, acceptable).
        let _ = tx.send(Frame::ok(ctx.call_id, result)).await;
        Ok(())
    }
}
