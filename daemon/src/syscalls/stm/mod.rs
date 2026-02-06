//! STM Namespace - Short-Term Memory syscalls for agent context
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides Short-Term Memory (STM) management for "head" agents,
//! enabling them to maintain session-scoped context across task executions. Unlike
//! the frame store (which is append-only and global), STM is mutable and scoped
//! per agent instance.
//!
//! **Core concepts:**
//! - **STM scope**: Each "head/<id>" agent has isolated STM (no cross-agent visibility)
//! - **Storage location**: Workspace-local files (`.abbot/memory/head-<id>.txt`)
//! - **Lifecycle**: STM persists across task invocations but is cleared on workspace reset
//! - **Operations**: `stm:read` (get current STM), `stm:update` (set/append/clear)
//!
//! **Integration points:**
//! - LLM context builders prepend STM to system prompts for conversation continuity
//! - Agents use STM to remember decisions, track progress, or cache lookups
//! - STM is NOT replicated to other agents (unlike frame store which is globally visible)
//!
//! **Persistence strategy:**
//! STM is stored in workspace-local files (not SQLite) for simplicity and performance.
//! Legacy migration: first read attempts to load from SQLite (`store.get_head_stm()`)
//! and migrates to file-based storage transparently.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Agent-scoped isolation**: Each head agent has private STM (no leakage between agents)
//! - **Mutable by design**: Unlike append-only frames, STM can be set/appended/cleared
//!   to reflect evolving agent state
//! - **Workspace-local persistence**: STM lives in `.abbot/memory/` directory, making it
//!   easy to inspect, backup, or clear alongside workspace state
//! - **Fail-safe defaults**: Missing STM files return empty string (not an error) to
//!   simplify first-run experience
//!
//! STM vs. FRAMES vs. LTM
//! ======================
//! This namespace is part of a three-tier memory architecture:
//!
//! **SHORT-TERM MEMORY (STM)**: Session-scoped, mutable, agent-private
//! - Use for: Current task context, temporary decisions, scratch space
//! - Lifecycle: Cleared on workspace reset or explicit `stm:update op=clear`
//! - Storage: Workspace-local files (`.abbot/memory/head-<id>.txt`)
//! - Access: `stm:read`, `stm:update` (head agents only)
//!
//! **FRAMES (Audit Trail)**: Global, append-only, immutable
//! - Use for: Conversation history, task tracking, debugging
//! - Lifecycle: Persists forever in `frames.db` (until manual cleanup)
//! - Storage: SQLite with indexed metadata columns
//! - Access: `frames:append`, `frames:select` (all actors)
//!
//! **LONG-TERM MEMORY (LTM)**: Cross-session, structured, queryable
//! - Use for: Facts, summaries, learned patterns (future: vector embeddings)
//! - Lifecycle: Persists across workspace resets (user-managed)
//! - Storage: SQLite or vector store (implementation TBD)
//! - Access: `ltm:*` syscalls (future)
//!
//! CONCURRENCY
//! ===========
//! - File-based storage uses atomic writes (`atomic_write_file_0600()`) for safety
//! - No locking required (each head agent has isolated STM file)
//! - Read operations are non-blocking (simple file read)
//! - TRADE-OFF: No cross-agent coordination (intentional isolation)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Files vs. SQLite**
//!    - CHOSEN: Files in `.abbot/memory/` directory
//!    - WHY: Simpler implementation, easier to inspect/backup, no schema migrations
//!    - COST: No structured querying (unlike frames which support SQL filters)
//!
//! 2. **Agent-scoped vs. global STM**
//!    - CHOSEN: Isolated per head agent (`head/<id>`)
//!    - WHY: Prevents confusion from interleaved state across parallel agents
//!    - COST: No shared state for multi-agent coordination (use frames for that)
//!
//! 3. **Workspace-local vs. global persistence**
//!    - CHOSEN: Workspace-local (cleared on workspace reset)
//!    - WHY: STM is tied to current task context, not long-term facts
//!    - COST: STM is lost if workspace is deleted (intentional - use LTM for persistence)
//!
//! WHO CAN USE
//! ===========
//! - `stm:read` - Any actor with `head/<id>` prefix (reads own STM)
//! - `stm:update` - Head agents only (requires mutation permission + actor check)

pub mod read;
mod update;

pub use read::StmRead;
pub use update::StmUpdate;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

/// Register short-term memory syscalls with the kernel dispatcher.
///
/// WHY: Encapsulates syscall registration to keep mod.rs focused on namespace overview.
/// Registered syscalls: `stm:read`, `stm:update`
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(StmRead::new()));
    dispatcher.register(Arc::new(StmUpdate::new()));
}
