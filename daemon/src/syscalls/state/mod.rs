//! State Namespace - Introspection queries for kernel runtime state
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides read-only introspection syscalls for querying kernel runtime state
//! stored in the SQLite-backed history Store. It enables agents and debugging tools to query:
//!
//! - **Wants**: Aspirational goals in the pool (managed by Mind/Room agents)
//! - **Hand execution logs**: Tool call history for completed tasks
//! - **Runtime statistics**: Aggregate counts and metadata
//!
//! **Storage architecture:**
//! - Backed by `history::Store` (SQLite database at `<workspace>/.abbot/store.db`)
//! - Persistent across kernel restarts (SQLite file-based storage)
//! - No caching layer (queries hit SQLite directly)
//!
//! **State scope model:**
//! - `wants` table: Global state (all agents share wants pool)
//! - `hand_exec` table: Task-scoped state (execution logs per task_id)
//! - `room_schedules` table: Global state (room scheduling metadata)
//! - No session-level or agent-level state isolation in this namespace
//!
//! **Integration points:**
//! - `Kernel::get()` for kernel singleton access
//! - `Kernel::store()` for SQLite Store handle
//! - `SyscallContext` for cancellation and actor context
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with query results as JSON arrays
//! - Returns `KernelError` for invalid modes, missing Store, or SQLite errors
//!
//! SECURITY MODEL
//! ==============
//! This namespace is **read-only introspection** with no mutation guard:
//!
//! 1. **No Mutation Permission Required**
//!    - WHY: Read-only queries have no side effects (safe for all agent types)
//!    - IMPLICATION: "Head", "hand", and "room" agents may all query state
//!    - RATIONALE: Introspection enables debugging and adaptive behavior
//!
//! 2. **Global State Access (No Scoping)**
//!    - WHY: All agents see the same wants pool and execution logs
//!    - HOW: No actor-based filtering in SQL queries
//!    - TRADE-OFF: No privacy isolation (one agent can see another's logs)
//!    - ACCEPTABLE: Within single-user workspace, no multi-tenant security model
//!
//! 3. **Output Limiting**
//!    - WHY: Prevents memory exhaustion from unbounded result sets
//!    - HOW: `limit` parameter clamped to 1-100 rows (default 20)
//!    - IMPLICATION: Large datasets require pagination (not yet implemented)
//!
//! 4. **No Write/Delete Operations**
//!    - WHY: Introspection should not mutate state (audit trail preservation)
//!    - HOW: Only SELECT queries issued (no INSERT/UPDATE/DELETE)
//!    - IMPLICATION: State cleanup requires separate syscalls or manual DB access
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Read-only introspection**: Queries should never mutate state
//! - **SQLite as single source of truth**: No in-memory caching (consistency over speed)
//! - **Mode-based dispatch**: Single syscall with `mode` parameter (extensible design)
//! - **Output limiting**: Default 20 rows, max 100 (prevents runaway queries)
//! - **Graceful deprecation**: Removed modes return helpful error messages (not panics)
//!
//! WHY MODE-BASED DESIGN:
//! - Single syscall namespace reduces kernel registration overhead
//! - Easier to add new introspection modes (just add match arm)
//! - Consistent query interface (all modes use `limit`, similar result format)
//! - Alternative rejected: One syscall per mode (e.g., `state:wants`, `state:logs`)
//!   would clutter syscall registry and require more boilerplate
//!
//! PERSISTENCE
//! ===========
//! All state in this namespace is **SQLite-backed and persistent**:
//!
//! **Wants pool:**
//! - Table: `wants` (id, want, context, priority, source, created_at)
//! - Survives kernel restart: YES (SQLite file)
//! - Cleanup strategy: Manual (no automatic expiration)
//!
//! **Hand execution logs:**
//! - Table: `hand_exec` (task_id, step, tool, args, output, success, timestamp)
//! - Survives kernel restart: YES (SQLite file)
//! - Cleanup strategy: Manual (no automatic expiration)
//! - Query pattern: By task_id (one task's complete execution history)
//!
//! **Statistics:**
//! - Computed via COUNT(*) aggregation (no cached values)
//! - Always reflects current SQLite state
//!
//! CONCURRENCY
//! ===========
//! - SQLite Store uses `Mutex<Connection>` for thread-safety (single writer, blocking)
//! - Read queries acquire lock briefly (no long-running transactions)
//! - Safe for concurrent execution across multiple task lanes
//! - No deadlock risk (queries complete in microseconds-milliseconds)
//!
//! PERFORMANCE
//! ===========
//! - SQLite queries with indexes: Sub-millisecond for typical workloads (<1000 rows)
//! - No N+1 query problem: Single query per mode (batch fetching)
//! - Output truncation: `hand_exec.output` limited to 200 chars (prevents large result JSON)
//! - Bottleneck: SQLite lock contention under heavy concurrent introspection
//!
//! TRADE-OFFS
//! ==========
//! 1. **Global State Access vs. Actor Scoping**
//!    - CHOSEN: Global access (all agents see same state)
//!    - REJECTED: Actor-based filtering (WHERE actor = ctx.actor)
//!    - WHY: Single-user workspace model, no multi-tenant security requirements
//!    - IMPLICATION: Agents can see each other's execution logs (acceptable for debugging)
//!
//! 2. **Mode-Based Dispatch vs. Separate Syscalls**
//!    - CHOSEN: Single `state:query` with `mode` parameter
//!    - REJECTED: `state:wants`, `state:logs`, `state:stats` as separate syscalls
//!    - WHY: Reduces syscall registration boilerplate, easier to extend
//!    - IMPLICATION: Callers must know valid modes (not type-safe at compile time)
//!
//! 3. **No Pagination**
//!    - CHOSEN: Clamp limit to 1-100, no offset/cursor support
//!    - WHY: Simple implementation, sufficient for typical debugging use cases
//!    - IMPLICATION: Cannot query more than 100 rows (acceptable for introspection)
//!
//! 4. **No Caching**
//!    - CHOSEN: Query SQLite directly on every call
//!    - REJECTED: In-memory cache with invalidation
//!    - WHY: Consistency guarantees (always see latest state), simpler implementation
//!    - IMPLICATION: Slower than cached reads, but acceptable for infrequent introspection
//!
//! REGISTERED SYSCALLS
//! ===================
//! - `state:query` - Query kernel runtime state with mode-based dispatch
//!
//! DEPRECATED MODES
//! ================
//! The following introspection modes were removed (bus/store.db message history deprecated):
//! - `messages` - Message history removed (Frame-based communication replaces bus)
//! - `needs` - Needs tracking removed (wants pool is primary state)
//! - `tasks` / `goals` - Task tracking removed (external task manager replaces kernel state)
//!
//! These modes now return helpful `E_INVALID_ARGS` errors explaining deprecation.

mod query;

pub use query::StateQuery;

use crate::kernel::KernelDispatcher;
use std::sync::Arc;

/// Register all state namespace syscalls with the kernel dispatcher.
///
/// WHY: Centralized registration ensures consistent initialization order
/// and makes it easy to audit which state introspection syscalls are available.
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(StateQuery::new()));
}
