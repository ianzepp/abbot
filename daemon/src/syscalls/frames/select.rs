//! Frames:Select - Query frame audit trail with flexible filtering
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides a powerful query interface for the kernel's frame store,
//! allowing clients to retrieve frames with filtering on multiple dimensions:
//! sequence range, timestamp range, operation type, kind, actor, scope, and more.
//!
//! **Query capabilities:**
//! - Time-based filtering: `since_ts_ms`, `until_ts_ms` (wall-clock time)
//! - Sequence-based pagination: `since_seq`, `until_seq` (monotonic frame counter)
//! - Metadata filtering: `ops` (req/ok/error), `kinds` (chat:user, log, etc.), `actors`
//! - Scope isolation: `scope` parameter for session/conversation filtering
//! - Full-text search: `query` parameter for text matching across frame JSON
//!
//! **Integration with FrameStore:**
//! - Uses SQLx pool from FrameStore for queries
//! - Uses `build_frame_select_sql()` to construct dynamic WHERE clauses
//! - Extracts indexed columns (seq, ts_ms, op, kind, scope, actor) for efficient filtering
//! - Optionally includes full `Frame` struct or raw JSON in response
//!
//! **Response format:**
//! - Emits `Frame::item` for each matching frame (up to `limit`)
//! - Returns `Frame::ok` with `{count, next_since_seq}` for pagination
//! - Metadata includes all indexed fields plus optional frame/frame_json
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Flexible filtering**: Support diverse query patterns (conversation history, debugging,
//!   task tracking) without specialized syscalls for each use case
//! - **Pagination-friendly**: `next_since_seq` enables efficient cursor-based pagination
//!   without OFFSET (which is slow on large tables)
//! - **Pool-based queries**: Uses shared SQLx connection pool for concurrent query safety
//! - **Fail-safe defaults**: `limit` clamped to [1, 2000] to prevent runaway queries
//!
//! CONCURRENCY
//! ===========
//! - Uses SQLx connection pool (safe for concurrent queries)
//! - Writer task uses separate pool connection (no read-write blocking)
//! - Cancellation checked between row fetches (long queries are interruptible)
//!
//! PERFORMANCE
//! ===========
//! - Indexed columns (seq, ts_ms, op, kind, scope, actor) enable fast WHERE filtering
//! - Default limit (200) prevents loading entire audit log into memory
//! - Ordering by `seq` leverages primary key index (no filesort)
//! - TRADE-OFF: Full-text search via `query` parameter may be slow on large databases
//!   (no FTS index currently)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Connection pool vs. per-query connection**
//!    - CHOSEN: Shared pool from FrameStore
//!    - WHY: Eliminates per-query connection overhead, enables concurrent queries
//!    - COST: Pool contention under heavy load (mitigated by max_connections=4)
//!
//! 2. **Pagination via sequence vs. OFFSET**
//!    - CHOSEN: Sequence-based pagination (`since_seq`)
//!    - WHY: OFFSET becomes slow on large tables, sequence-based is O(1) with index
//!    - COST: Clients must track last sequence number (handled by API layer)
//!
//! 3. **Optional frame inclusion**
//!    - CHOSEN: `include_frame` and `include_json` flags (defaults: true, false)
//!    - WHY: Metadata-only queries are faster, but full frames needed for reconstruction
//!    - COST: Callers must explicitly request full frame data when needed
//!
//! WHO CAN USE
//! ===========
//! - Any actor (no permission check)
//! - Queries are typically scoped to prevent cross-session leakage
//! - Used by LLM context builders, debugging tools, conversation history APIs

use async_trait::async_trait;
use serde_json::json;
use sqlx::Row;
use tokio::sync::mpsc;

use crate::kernel::frame_select::{build_frame_select_sql, execute_frame_select};
use crate::kernel::{Frame, FrameSelectArgs, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for querying the frame audit trail with flexible filtering.
///
/// WHY: Provides a unified query API for conversation history, task tracking,
/// debugging, and agent memory without specialized syscalls for each use case.
pub struct FramesSelect;

impl Default for FramesSelect {
    fn default() -> Self {
        Self::new()
    }
}

impl FramesSelect {
    /// Create a new `FramesSelect` syscall.
    ///
    /// WHY: Zero-state syscall (queries are read-only and stateless).
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for FramesSelect {
    fn name(&self) -> &'static str {
        "frames:select"
    }

    /// Query frames from the audit trail with filtering and pagination.
    ///
    /// WHY: Enables conversation history reconstruction, task tracking, debugging,
    /// and agent memory retrieval via flexible filtering on metadata dimensions.
    ///
    /// ARGUMENTS (all optional):
    /// - `scope`: Filter by scope/session (e.g., "main", "task/abc123")
    /// - `since_seq`, `until_seq`: Sequence range for pagination
    /// - `since_ts_ms`, `until_ts_ms`: Timestamp range for time-based queries
    /// - `limit`: Max results (default 200, clamped to 1-2000)
    /// - `order`: "asc" or "desc" (default "asc")
    /// - `ops`: Filter by operation type (["req", "ok", "error"])
    /// - `kinds`: Filter by kind metadata (["chat:user", "log", "progress"])
    /// - `actors`: Filter by actor (["head/agent1", "hand/llm"])
    /// - `parent_id`, `frame_id`, `reply_to`: Filter by frame relationships
    /// - `query`: Full-text search across frame JSON (slow, no FTS index)
    /// - `include_frame`: Include full Frame struct (default true)
    /// - `include_json`: Include raw JSON string (default false)
    ///
    /// RETURNS:
    /// - `Frame::item` for each matching frame with metadata + optional frame/JSON
    /// - `Frame::ok` with `{count, next_since_seq}` for pagination continuation
    ///
    /// CONCURRENCY NOTE: Uses SQLx pool for concurrent query safety.
    /// Cancellation checked between row fetches for interruptibility.
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // -------------------------------------------------------------------------
        // PHASE 1: Argument Parsing & Validation
        // WHY: Parse query arguments and apply safe defaults (limit clamping, order
        // validation) before constructing SQL to prevent malformed queries.
        // -------------------------------------------------------------------------
        let args: FrameSelectArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let store = k
            .frames()
            .ok_or_else(|| KernelError::internal("frame store not initialized"))?;

        // WHY: Clamp limit to prevent runaway queries from loading entire audit log.
        // Default 200 balances pagination granularity with query overhead.
        let limit = args.limit.unwrap_or(200).clamp(1, 2000) as i64;
        let include_frame = args.include_frame.unwrap_or(true);
        let include_json = args.include_json.unwrap_or(false);

        // WHY: Normalize order to uppercase for SQL compatibility and reject invalid values.
        let order = match args
            .order
            .as_deref()
            .unwrap_or("asc")
            .to_lowercase()
            .as_str()
        {
            "asc" => "ASC",
            "desc" => "DESC",
            other => {
                return Err(KernelError::invalid_args(format!(
                    "invalid order '{other}' (expected 'asc' or 'desc')"
                )));
            }
        };

        // -------------------------------------------------------------------------
        // PHASE 2: SQL Query Construction
        // WHY: Delegate to build_frame_select_sql() to construct dynamic WHERE
        // clauses based on provided filters. This centralizes query logic and
        // enables testing without duplicating SQL construction.
        // -------------------------------------------------------------------------
        let (sql, params) = build_frame_select_sql(&args, order, limit);

        // -------------------------------------------------------------------------
        // PHASE 3: SQLx Query Execution
        // WHY: Use connection pool from FrameStore for concurrent query safety.
        // Extract indexed columns and optionally parse full Frame struct from JSON.
        // -------------------------------------------------------------------------
        let rows = execute_frame_select(store.pool(), &sql, &params)
            .await
            .map_err(KernelError::io)?;

        let mut items: Vec<serde_json::Value> = Vec::new();
        let mut max_seq: u64 = 0;
        for row in &rows {
            // WHY: Check cancellation between rows to allow interrupting long queries.
            ctx.check_cancelled()?;

            // WHY: Extract all indexed columns into structured metadata. This avoids
            // parsing JSON for common query patterns (filtering by actor, kind, etc.).
            let seq: i64 = row.get(0);
            let ts_ms: i64 = row.get(1);
            let op: String = row.get(2);
            let name: Option<String> = row.try_get(3).ok();
            let actor: Option<String> = row.try_get(4).ok();
            let frame_id: String = row.get(5);
            let parent_id: Option<String> = row.try_get(6).ok();
            let scope: Option<String> = row.try_get(7).ok();
            let kind: Option<String> = row.try_get(8).ok();
            let reply_to: Option<String> = row.try_get(9).ok();
            let frame_json: String = row.try_get(10).unwrap_or_else(|_| "{}".to_string());

            let seq_u = seq.max(0) as u64;
            max_seq = max_seq.max(seq_u);

            let mut out = json!({
                "seq": seq_u,
                "ts_ms": ts_ms,
                "op": op,
                "name": name,
                "actor": actor,
                "frame_id": frame_id,
                "parent_id": parent_id,
                "scope": scope,
                "kind": kind,
                "reply_to": reply_to,
            });

            // WHY: Optionally include full Frame struct for conversation reconstruction.
            // Default is true because most queries need full frame data.
            if include_frame {
                let frame: Frame = serde_json::from_str(&frame_json).unwrap_or_else(|_| {
                    Frame::error(
                        ctx.call_id,
                        json!({"code": "E_LOG_PARSE", "message": "failed to parse frame"}),
                    )
                });
                out["frame"] = serde_json::to_value(&frame).unwrap_or(serde_json::Value::Null);
            }
            // WHY: Optionally include raw JSON for debugging (rare use case).
            if include_json {
                out["frame_json"] = serde_json::Value::String(frame_json);
            }

            items.push(out);
        }

        // -------------------------------------------------------------------------
        // PHASE 4: Response Emission
        // WHY: Stream results as Frame::item messages for incremental processing,
        // then emit Frame::ok with pagination metadata (next_since_seq).
        // -------------------------------------------------------------------------
        let count = items.len() as u64;

        for item in items {
            ctx.check_cancelled()?;
            let _ = tx.send(Frame::item(ctx.call_id, item)).await;
        }

        // WHY: Include next_since_seq for cursor-based pagination (more efficient
        // than OFFSET on large tables).
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"count": count, "next_since_seq": max_seq}),
            ))
            .await;
        Ok(())
    }
}
