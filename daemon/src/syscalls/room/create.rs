//! Room:Create - Allocate room ID and register with RoomCoordinator
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall allocates a new room ID (UUID v4) and registers the room with the
//! RoomCoordinator's internal tracking structures. It's the first step in the room
//! lifecycle before opening a stream or running deliberation.
//!
//! **Room types supported:**
//! - `conclave`: Strategic planning sessions (5 rounds max, full participant set)
//! - `autonomy`: Quick reflection sessions (3 rounds max, lighter weight)
//! - `work`: Task-focused collaboration with git worktree isolation (10 rounds max)
//!
//! **Scope semantics:**
//! - Rooms are scoped to logical contexts (e.g., "main", "experiment", "bugfix")
//! - Scope affects which context files are loaded during deliberation
//! - Default scope is "main" if unspecified
//!
//! **Integration points:**
//! - `RoomCoordinator::create()` - Registers room in coordinator's tracking map
//! - Returns room_id for subsequent `room:stream` and `room:run` calls
//! - Room metadata (type, scope, creation time) is tracked but not persisted until
//!   `room:schedule` is called
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Explicit lifecycle**: Room creation is separate from execution so callers can
//!   set up streams before running (enables real-time observation of deliberation).
//! - **Lightweight allocation**: Room:create only allocates ID and registers metadata,
//!   no expensive resources (worktrees, LLM queries) are provisioned until room:run.
//! - **Permissive access**: Any actor can create rooms (no permission checks) since
//!   rooms are internal coordination mechanisms not exposed to external callers.
//!
//! CONCURRENCY
//! ===========
//! - Safe for concurrent execution (RoomCoordinator uses internal synchronization)
//! - Multiple rooms can be created simultaneously without coordination
//! - Room IDs are UUIDs (collision probability negligible)
//!
//! SECURITY MODEL
//! ==============
//! - No permission checks (any actor can create rooms)
//! - WHY permissive: Rooms are internal coordination mechanisms. The RoomCoordinator
//!   controls when rooms actually execute based on idle detection policy.
//! - Room creation does not allocate expensive resources (deferred to room:run)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, RoomKind, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for allocating room IDs and registering with RoomCoordinator.
///
/// WHY: Separates room allocation from execution so callers can open streams
/// before running deliberation (enables real-time observation).
pub struct RoomCreate;

impl RoomCreate {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomCreate {
    fn name(&self) -> &'static str {
        "room:create"
    }

    /// Allocate a new room ID and register with coordinator.
    ///
    /// WHY this exists: Explicit room creation separates allocation from execution,
    /// enabling callers to set up streams before running (critical for observing
    /// deliberation progress in real-time).
    ///
    /// ARGUMENTS:
    /// - `type` or `kind`: Room type ("conclave", "autonomy", "work")
    /// - `scope`: Logical context for the room (default: "main")
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{room_id, type, scope}` on success
    /// - `E_INVALID_ARGS` if type is invalid or scope is empty
    /// - `E_INTERNAL` if kernel not initialized
    ///
    /// USAGE:
    /// ```json
    /// {"type": "conclave", "scope": "main"}
    /// ```
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        // WHY accept both "type" and "kind": Backward compatibility with existing
        // callers that may use either field name. "type" is preferred but "kind"
        // is supported for historical JSON schemas.
        let kind = data
            .get("type")
            .or_else(|| data.get("kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let Some(kind) = RoomKind::from_str(kind) else {
            return Err(KernelError::invalid_args(
                "room type must be 'conclave', 'autonomy', or 'work'",
            ));
        };

        // WHY default to "main": Most rooms operate in primary workspace context.
        // Explicit scopes enable experimental or feature-branch isolation without
        // affecting main workspace deliberations.
        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        // WHY RoomCoordinator::create: Registers room in coordinator's tracking map
        // and returns newly allocated UUID. Room is not persisted to SQLite until
        // room:schedule is called (ephemeral rooms don't need persistence).
        let room_id = k.rooms().create(kind, scope).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"room_id": room_id.to_string(), "type": kind.as_str(), "scope": scope}),
            ))
            .await;
        Ok(())
    }
}
