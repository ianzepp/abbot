//! Session Namespace - Runtime session state management syscalls
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `session` namespace provides syscalls for managing runtime session state that
//! persists across kernel restarts. Session state is stored in SQLite via the kernel's
//! history store and includes:
//!
//! - **Model preferences** (`session_model` table) - LLM model selection per room
//! - **Environment variables** (`session_env` table) - Session-scoped env vars (future)
//! - **Session state** (`session_state` table) - Arbitrary key-value state (future)
//!
//! **Room names:**
//! - `"main"` - Primary CLI session (single-user default)
//! - `"<derived-hash>"` - Derived room names for multi-user servers (isolated per user)
//!
//! **Integration points:**
//! - `Store` (SQLite-backed persistence in `history.db`)
//! - `SyscallContext` (mutation guards, room identification)
//! - `Kernel::get()` (global kernel instance access)
//!
//! **Registered syscalls:**
//! - `session:model_set` - Switch LLM model for a room (requires "head" actor)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Persistent state**: Session preferences survive kernel restarts (SQLite storage)
//! - **Room isolation**: Each room has independent state (no cross-contamination)
//! - **Mutation guards**: Only "head" agents may modify session state
//! - **Deferred validation**: Model names are validated at LLM runtime, not syscall time
//! - **Minimal API surface**: Currently only model switching; designed for future expansion
//!
//! SECURITY MODEL
//! ==============
//! Session state modification is restricted to prevent privilege escalation:
//!
//! 1. **Actor Authorization**
//!    - WHY: Session state affects system-wide behavior (e.g., which LLM model is used)
//!    - HOW: All mutating session syscalls enforce `ctx.require_mutation()` (only "head")
//!    - ATTACK PREVENTED: "Hand" agents (LLM-controlled) cannot modify session configuration
//!
//! 2. **Scope Isolation**
//!    - WHY: Multi-user servers need per-room configuration without cross-talk
//!    - HOW: SQLite tables use `room TEXT PRIMARY KEY` for room-scoped state
//!    - ATTACK PREVENTED: Rooms cannot interfere with each other's state
//!
//! 3. **No Network Operations**
//!    - WHY: Session state is local-only (no remote APIs, no data exfiltration vectors)
//!    - HOW: All syscalls operate on local SQLite database (`history.db`)
//!    - ATTACK PREVENTED: Session state cannot be exfiltrated via network requests
//!
//! WHY SESSION STATE IS MUTABLE
//! =============================
//! Session state is intentionally mutable (vs. immutable context) because:
//!
//! 1. **User Intent**
//!    - Users explicitly request model switching via commands (not immutable config)
//!    - Changes should persist across kernel restarts (not ephemeral like context)
//!
//! 2. **Runtime Flexibility**
//!    - Model selection varies by task (cheap model for exploration, expensive for production)
//!    - Cannot predict optimal model at kernel startup time (task-dependent)
//!
//! 3. **Multi-Tenancy**
//!    - Server deployments need per-session configuration (different users, different models)
//!    - Immutable context would require separate kernel instances per session (wasteful)
//!
//! 4. **Persistence Requirements**
//!    - Session preferences should survive kernel crashes/restarts (user expectation)
//!    - Immutable context is lost on restart; mutable state persists (SQLite durability)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Mutable State vs. Immutable Context**
//!    - CHOSEN: Mutable session state (persisted in SQLite)
//!    - REJECTED: Immutable context (passed at kernel startup)
//!    - WHY: Session preferences change at runtime (user commands, A/B testing)
//!    - IMPLICATION: Requires mutation guards to prevent unauthorized state changes
//!
//! 2. **SQLite vs. In-Memory State**
//!    - CHOSEN: SQLite persistence (`history.db`)
//!    - REJECTED: In-memory HashMap (lost on restart)
//!    - WHY: Users expect model preferences to persist across kernel restarts
//!    - IMPLICATION: I/O overhead for state changes (acceptable, infrequent operations)
//!
//! 3. **Minimal Validation vs. Strict Validation**
//!    - CHOSEN: Minimal validation at syscall time (empty string checks only)
//!    - REJECTED: Validate model names against provider catalogs
//!    - WHY: Model catalogs are provider-dependent and change frequently
//!    - IMPLICATION: Invalid model names accepted by syscall, fail at LLM runtime
//!
//! FUTURE EXPANSION
//! ================
//! Planned session syscalls (not yet implemented):
//! - `session:env_set` - Set session-scoped environment variables
//! - `session:env_get` - Retrieve session-scoped environment variables
//! - `session:state_set` - Set arbitrary session state (key-value pairs)
//! - `session:state_get` - Retrieve arbitrary session state
//! - `session:clear` - Clear all state for a room

mod model_set;

pub use model_set::SessionModelSet;

use crate::kernel::KernelDispatcher;
use std::sync::Arc;

/// Register all session namespace syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration for the session namespace, ensuring
/// consistent initialization order and discoverability.
///
/// REGISTERED SYSCALLS:
/// - `session:model_set` - Switch LLM model for a room
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(SessionModelSet::new()));
}
