//! Want - Aspirational goal pool management for room/mind agents
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `want` namespace implements a persistent pool of aspirational goals (wants) that room/mind
//! agents use to guide their planning and decision-making. Wants represent desirable outcomes or
//! improvements the agent aspires to achieve, distinct from immediate tasks (needs) or concrete
//! work items (tasks).
//!
//! **Core concepts:**
//! - **Wants**: High-level aspirational goals ("improve error messages", "refactor auth layer")
//! - **Pool management**: SQLite-backed persistent storage in `wants` table
//! - **Priority levels**: Categorize wants by importance (normal, high, urgent)
//! - **Promotion**: Convert wants into actionable needs via `want:promote`
//! - **Room integration**: Mind/room agents query wants to guide deliberation topics
//!
//! **Integration points:**
//! - `Store.wants` table - Persistent storage for want pool (survives kernel restart)
//! - `RoomCoordinator` - Queries wants to seed room deliberation agendas
//! - `need:enqueue` - Promoted wants become needs for immediate execution
//! - `state:query` - Introspection syscall can query want pool statistics
//!
//! **Want lifecycle:**
//! 1. **Creation** (want:create) - Add new want to pool with priority and context
//! 2. **Listing** (want:list) - Query current wants (ordered by priority/creation time)
//! 3. **Promotion** (want:promote) - Convert want to need, enqueue via need:enqueue
//! 4. **Removal** (want:remove) - Delete want from pool (manual cleanup)
//!
//! **Registered syscalls:**
//! - `want:list` - Enumerate wants in pool (with pagination)
//! - `want:create` - Add new want to pool
//! - `want:remove` - Delete want from pool
//! - `want:promote` - Convert want to need and dispatch via need:enqueue
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Aspirational vs. actionable**: Wants are goals, not tasks (conceptual vs. concrete)
//! - **Persistent pool**: Wants survive kernel restart (SQLite storage)
//! - **Priority-based organization**: High-priority wants are more likely to be promoted
//! - **Manual curation**: No automatic expiration (room agents decide what to pursue)
//! - **Promotion workflow**: Explicit conversion to needs (wants don't execute automatically)
//! - **Context preservation**: Each want includes context explaining why it's desirable
//!
//! WHY WANTS EXIST
//! ===============
//! Agents need a way to track aspirational improvements without cluttering the immediate
//! work queue (needs) or task delegation system (tasks). Wants solve this by providing:
//!
//! 1. **Long-term planning**: Room agents can deliberate on wants during idle periods
//! 2. **Priority management**: Not all improvements are equally important
//! 3. **Context retention**: Capture rationale for why something is desirable
//! 4. **Deferred execution**: Keep good ideas visible without committing resources
//!
//! **Example wants:**
//! - "Improve error messages in auth module" (normal priority)
//! - "Add integration tests for payment flow" (high priority)
//! - "Refactor config loading to support YAML" (normal priority)
//! - "Fix critical security vulnerability in auth" (urgent priority → promote immediately)
//!
//! WANT vs. NEED vs. TASK
//! ======================
//! Abbot has three complementary coordination primitives:
//!
//! **WANT (this namespace)**: Aspirational goals
//! - Purpose: Capture desirable improvements for future consideration
//! - Storage: SQLite `wants` table (persistent)
//! - Execution: Manual promotion to needs by room/mind agents
//! - Use case: Long-term planning, deferred improvements
//!
//! **NEED (need namespace)**: Urgent coordination
//! - Purpose: Immediate work items requiring attention
//! - Storage: In-memory priority queue (ephemeral)
//! - Execution: Lease by agents, fulfill on completion
//! - Use case: High-priority coordination, urgent tasks
//!
//! **TASK (task namespace)**: Delegated work items
//! - Purpose: Head agent delegates work to hand agents
//! - Storage: In-memory room-based queues (ephemeral)
//! - Execution: Lease by hand agents, complete with status
//! - Use case: Project execution, multi-step workflows
//!
//! **When to use wants:**
//! - "This would be nice to do someday" → create want
//! - "We should improve X when we have time" → create want
//! - Room agent identifies improvement during deliberation → create want
//!
//! **When to promote want → need:**
//! - User explicitly requests it ("do this now")
//! - Room reaches consensus to pursue it
//! - Want priority is urgent (immediate action required)
//!
//! PROMOTION WORKFLOW
//! ==================
//! `want:promote` implements a two-step atomic operation:
//!
//! 1. **Remove from want pool** (DELETE FROM wants WHERE id = ?)
//!    - WHY: Prevent duplicate promotion (want can only be promoted once)
//!    - Atomic SQL operation ensures no race conditions
//!
//! 2. **Dispatch via need:enqueue** (INSERT INTO needs queue)
//!    - WHY: Converted want becomes actionable need for immediate execution
//!    - Priority preserved (urgent want → urgent need)
//!    - Context carried forward (want.context → need.context)
//!
//! **Idempotency:** Promoting non-existent want_id returns `{"promoted": false, "reason": "want not found"}`
//! instead of error, enabling safe retries.
//!
//! **Fire-and-forget dispatch:** Promotion dispatches need but doesn't wait for completion.
//! This prevents want:promote from blocking on need execution (needs may take minutes).
//!
//! PERSISTENCE MODEL
//! =================
//! **SQLite storage (`wants` table):**
//! - Columns: `id TEXT PRIMARY KEY, want TEXT, context TEXT, priority TEXT, source TEXT, created_at INTEGER`
//! - Indexed by: `id` (primary key), `created_at` (chronological order)
//! - Survives: Kernel restart, workspace changes
//! - Cleanup: Manual (no automatic expiration)
//!
//! **Priority levels:**
//! - "normal" - Default priority for routine improvements
//! - "high" - Important but not urgent
//! - "urgent" - Requires immediate attention (auto-promotes in some scenarios)
//!
//! **Source tracking:**
//! - Currently hardcoded to "mind" (room/mind agents create wants)
//! - Future: Track which agent/user created each want
//!
//! SECURITY MODEL
//! ==============
//! **No actor restrictions:**
//! - WHY: Want pool is aspirational planning (not destructive operations)
//! - All agents (head, hand, room) can create/list wants
//! - Only promotion (want:promote) has potential for resource consumption
//!
//! **No mutation guard:**
//! - want:create/remove/promote don't require mutation permission
//! - WHY: Wants don't modify filesystem or execute code
//! - Promotion dispatches need:enqueue which has its own authorization
//!
//! **Output limiting:**
//! - want:list clamped to 1-100 results (default 20)
//! - WHY: Prevents memory exhaustion from unbounded queries
//!
//! CONCURRENCY
//! ===========
//! - SQLite Store uses `Mutex<Connection>` for thread-safety
//! - All want operations acquire lock briefly (no long transactions)
//! - Promotion is atomic (remove + enqueue as single operation)
//! - Safe for concurrent access across multiple agents
//!
//! PERFORMANCE
//! ===========
//! - Typical want pool size: 10-50 wants (small dataset)
//! - SQLite queries: <1ms for list/create/remove
//! - Promotion includes need:enqueue dispatch (~1-2ms total)
//! - No N+1 queries (batch fetching via LIMIT clause)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Manual Curation vs. Automatic Expiration**
//!    - CHOSEN: No automatic cleanup (manual removal only)
//!    - REJECTED: Age-based expiration (e.g., delete after 30 days)
//!    - WHY: Good ideas remain relevant indefinitely (not time-bound)
//!    - IMPLICATION: Want pool may grow large (room agents should periodically review)
//!
//! 2. **Promotion Removes Want**
//!    - CHOSEN: Promoted wants are deleted from pool
//!    - REJECTED: Keep wants after promotion (mark as "promoted")
//!    - WHY: Prevents duplicate promotion, keeps pool focused on pending wants
//!    - IMPLICATION: Cannot track promotion history (use frames for audit trail)
//!
//! 3. **SQLite vs. In-Memory**
//!    - CHOSEN: SQLite persistence
//!    - WHY: Wants are long-term aspirations (should survive restart)
//!    - COST: Disk I/O overhead (~1ms per operation)
//!
//! 4. **No Want Editing**
//!    - CHOSEN: Create/remove only (no want:update syscall)
//!    - WHY: Simple API, encourages creating new want if context changes
//!    - IMPLICATION: Editing requires remove + create (loses creation timestamp)

mod create;
mod list;
mod promote;
mod remove;

pub use create::WantCreate;
pub use list::WantList;
pub use promote::WantPromote;
pub use remove::WantRemove;

use crate::kernel::KernelDispatcher;
use std::sync::Arc;

/// Register all want namespace syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration for the want namespace. Called during
/// kernel initialization to make want pool management syscalls available.
///
/// REGISTERED SYSCALLS:
/// - `want:list` - Enumerate wants in pool with pagination
/// - `want:create` - Add new want to pool with priority and context
/// - `want:remove` - Delete want from pool by ID
/// - `want:promote` - Convert want to need and dispatch via need:enqueue
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(WantList::new()));
    dispatcher.register(Arc::new(WantCreate::new()));
    dispatcher.register(Arc::new(WantRemove::new()));
    dispatcher.register(Arc::new(WantPromote::new()));
}
