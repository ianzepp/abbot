//! Frames Namespace - Frame storage and retrieval syscalls
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides syscalls for interacting with the kernel's frame store,
//! which maintains an immutable audit log of all kernel events in `frames.db`. The
//! frame store serves as both a debugging tool (complete kernel event trace) and
//! the foundation for conversation history, task tracking, and agent memory.
//!
//! **Core components:**
//! - `FrameStore` (`src/kernel/frame_store.rs`) - SQLite-backed append-only frame log
//! - `frames:append` - Emit arbitrary frames into the audit trail (primarily for logging)
//! - `frames:select` - Query frames with flexible filtering (scope, time, actor, kind)
//!
//! **Integration points:**
//! - All kernel syscalls emit frames (req/ok/error/event/done) which are auto-logged
//! - Chat operations (`chat:user`, `chat:head`) are stored as frames with kind metadata
//! - Task/need lifecycle events (enqueue, complete, fulfill) create queryable frame history
//! - LLM context builders query frames to reconstruct conversation threads
//!
//! **Persistence strategy:**
//! The frame store writes to `frames.db` (configurable via `config.paths.frames_db_path`)
//! with a dedicated writer thread to prevent blocking async operations. Each frame gets
//! a monotonic sequence number (`seq`) for ordering and deduplication.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Append-only immutability**: Frames are never modified or deleted, enabling reliable
//!   audit trails and simplifying concurrency (no read-write conflicts)
//! - **Structured metadata extraction**: Frames store JSON but extract key fields (op, kind,
//!   scope, actor) into indexed columns for efficient querying
//! - **Separation of concerns**: Frame logging is separate from conversation extraction (see
//!   `frame_select.rs` for conversation parsing) and short-term memory (see `stm/` namespace)
//! - **Fail-safe defaults**: `frames:append` never fails a syscall due to logging issues,
//!   preventing cascading failures from audit infrastructure
//!
//! CONCURRENCY
//! ===========
//! - **Single writer thread**: FrameStore spawns a dedicated blocking thread for SQLite writes,
//!   avoiding async runtime blocking and eliminating write contention
//! - **Read-only queries**: `frames:select` opens separate read-only connections, allowing
//!   concurrent queries without writer interference
//! - **Backpressure handling**: Append channel has bounded capacity (4096 frames) to prevent
//!   unbounded memory growth, with intentional blocking to preserve all frames
//!
//! TRADE-OFFS
//! ==========
//! 1. **SQLite vs. in-memory queue**
//!    - CHOSEN: SQLite for durability and queryability
//!    - REJECTED: Pure in-memory logging (lost on crash)
//!    - WHY: Durable audit trail is critical for debugging production issues
//!    - COST: Disk I/O overhead (~1ms per batch of writes)
//!
//! 2. **Append-only vs. mutable log**
//!    - CHOSEN: Append-only (no updates/deletes)
//!    - WHY: Simpler concurrency, reliable audit trail, easy replication
//!    - COST: Disk space grows unbounded (future: rotation/archival strategy)
//!
//! 3. **Structured fields vs. full JSON search**
//!    - CHOSEN: Extract op/kind/scope/actor to indexed columns
//!    - WHY: Fast filtering on common dimensions (scope, actor) without FTS overhead
//!    - COST: Schema changes require migration if new extracted fields needed
//!
//! WHO CAN USE
//! ===========
//! - `frames:append` - Any actor (no permission check), but emits events visible to all
//! - `frames:select` - Any actor can query, but queries are scoped (no cross-session leakage)

mod append;
mod select;

pub use append::FramesAppend;
pub use select::FramesSelect;

/// Register frame storage syscalls with the kernel dispatcher.
///
/// WHY: Encapsulates syscall registration to keep mod.rs focused on namespace overview.
/// Registered syscalls: `frames:append`, `frames:select`
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(FramesAppend::new()));
    dispatcher.register(Arc::new(FramesSelect::new()));
}
