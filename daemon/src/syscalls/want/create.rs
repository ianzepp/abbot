//! Want:Create - Add new want/goal to queue
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Creates a new want (goal/desire/intention) in the persistent want queue. Wants are
//! created by "mind" agents during reflection and represent aspirational goals that may
//! be promoted to executable "needs" later.
//!
//! **Integration:** Persists to SQLite via `Store::add_want()`. The want is stored with
//! a UUID, priority level, and source attribution ("mind"). Later, `want:promote` can
//! convert it to a need via `need:enqueue`.
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{"want_id": UUID, "priority": "normal"|"urgent", "status": "added"}`
//! - Returns `E_INVALID_ARGS` if want text is missing or malformed
//! - Returns `E_INTERNAL` if kernel or store is not initialized
//! - Returns `E_IO` if database insertion fails
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Mind-driven creation**: Only mind agents create wants (strategic reflection)
//! - **UUID assignment**: Server-side ID generation prevents collision/replay attacks
//! - **Priority defaulting**: Defaults to "normal" to avoid accidental urgent escalation
//! - **Source attribution**: Hardcoded "mind" source for future provenance tracking
//!
//! USE CASES
//! =========
//! - **Reflection output**: Mind agents record insights from room deliberations
//! - **Long-term planning**: Accumulate goals without immediate execution
//! - **Prioritization backlog**: Store ideas for later promotion

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `want:create` syscall.
///
/// WHY: Separates required field (want text) from optional metadata (context, priority).
#[derive(Debug, Deserialize)]
struct WantCreateArgs {
    /// The want/goal description.
    ///
    /// WHY: Required field - meaningless to create empty want.
    want: String,

    /// Additional context about the want.
    ///
    /// WHY: Optional - provides background information for later promotion decisions.
    #[serde(default)]
    context: String,

    /// Priority level ("normal" or "urgent").
    ///
    /// WHY: Defaults to "normal" to prevent accidental urgent escalation.
    #[serde(default)]
    priority: Option<String>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for creating new wants in the queue.
///
/// WHY: Stateless unit struct since want creation requires no configuration.
pub struct WantCreate;

impl WantCreate {
    /// Create a new `WantCreate` syscall.
    ///
    /// WHY: Standard constructor for consistency with other syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for WantCreate {
    fn name(&self) -> &'static str {
        "want:create"
    }

    /// Create a new want in the persistent queue.
    ///
    /// WHY: Enables mind agents to record goals/desires during reflection without
    /// immediately executing them. This separation allows prioritization and batching
    /// before promoting wants to executable needs.
    ///
    /// USE CASE: Invoked by mind agents after room deliberations to capture insights,
    /// action items, or strategic goals for later execution.
    ///
    /// SECURITY NOTE: No actor restrictions since wants are read-only until promoted.
    /// Even if a malicious actor creates many wants, they won't execute until explicitly
    /// promoted via `want:promote` (which could add actor checks in the future).
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"want_id": UUID, "priority": "normal"|"urgent", "status": "added"}`
    /// - `E_INVALID_ARGS` if want text is missing or malformed
    /// - `E_INTERNAL` if kernel or store is not initialized
    /// - `E_IO` if database insertion fails
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // WHY: Store is required for persistence
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        let args: WantCreateArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Default to "normal" priority to avoid accidental urgent escalation.
        // Mind agents must explicitly request urgent priority.
        let priority = args.priority.as_deref().unwrap_or("normal");

        // WHY: Server-side UUID generation prevents ID collision and replay attacks.
        // Client cannot specify want_id to avoid conflicts with existing wants.
        let want_id = Uuid::new_v4().to_string();

        // WHY: Source is hardcoded to "mind" for now. Future: could use ctx.actor
        // for finer-grained provenance tracking.
        store
            .add_want(&want_id, &args.want, &args.context, priority, "mind")
            .map_err(|e| KernelError::io(format!("failed to add want: {e}")))?;

        // WHY: Return want_id so caller can reference this want in future promote/remove calls
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "want_id": want_id,
                    "priority": priority,
                    "status": "added"
                }),
            ))
            .await;

        Ok(())
    }
}
