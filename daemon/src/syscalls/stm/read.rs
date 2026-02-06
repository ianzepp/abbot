//! STM:Read - Retrieve short-term memory for a head agent
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall retrieves the current Short-Term Memory (STM) for the calling
//! head agent. STM is stored in workspace-local files (`.abbot/memory/head-<id>.txt`)
//! and provides session-scoped context that persists across task executions.
//!
//! **Storage location:**
//! - Modern: `.abbot/memory/head-<id>.txt` (workspace-local file)
//! - Legacy: SQLite `head_stm` table (migrated on first read)
//!
//! **Actor extraction:**
//! The `head_id` is extracted from the syscall context's actor field, which must
//! be in the format `head/<id>`. This prevents "hand" or "room" agents from accessing
//! head-specific memory.
//!
//! **Migration strategy:**
//! On first read, if the file doesn't exist, we attempt to load from the legacy
//! SQLite location (`store.get_head_stm()`). If found, we migrate to file-based
//! storage transparently and return the migrated content.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Fail-safe defaults**: Missing STM files return empty string (not an error)
//! - **Transparent migration**: Legacy SQLite-based STM is migrated on first access
//! - **Actor-based isolation**: Only the owning head agent can read its STM
//! - **Workspace-local state**: STM is tied to workspace directory (cleared on reset)
//!
//! CONCURRENCY
//! ===========
//! - Read operations are non-blocking (simple file read)
//! - No locking required (each head agent has isolated STM file)
//! - Migration writes use atomic file operations for safety
//!
//! TRADE-OFFS
//! ==========
//! 1. **File-based vs. SQLite storage**
//!    - CHOSEN: Files in `.abbot/memory/` directory
//!    - WHY: Simpler implementation, easier to inspect/backup
//!    - COST: No structured querying (but STM is unstructured text anyway)
//!
//! 2. **Actor extraction vs. explicit head_id parameter**
//!    - CHOSEN: Extract from `ctx.actor` field
//!    - WHY: Prevents impersonation (actor is set by kernel, not caller)
//!    - COST: Callers can't read other agents' STM (intentional isolation)
//!
//! WHO CAN USE
//! ===========
//! - Only head agents (`head/<id>` actor format)
//! - Each agent can only read its own STM (no cross-agent access)

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for retrieving short-term memory for a head agent.
///
/// WHY: Enables head agents to maintain session-scoped context across task
/// executions (e.g., remembered decisions, progress tracking, cached lookups).
pub struct StmRead;

impl StmRead {
    /// Create a new `StmRead` syscall.
    ///
    /// WHY: Zero-state syscall (read operations are stateless).
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for StmRead {
    fn name(&self) -> &'static str {
        "stm:read"
    }

    /// Retrieve the short-term memory for the calling head agent.
    ///
    /// WHY: Provides session-scoped context that persists across task executions,
    /// enabling agents to remember decisions or track progress without polluting
    /// the global frame store.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{head_id, stm, len}` (stm is the full memory string)
    /// - Empty string if STM file doesn't exist (not an error)
    ///
    /// SECURITY NOTE: Actor must be `head/<id>` format. This prevents "hand" or
    /// "room" agents from accessing head-specific memory.
    ///
    /// MIGRATION: First read attempts to load from legacy SQLite location and
    /// transparently migrates to file-based storage if found.
    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // -------------------------------------------------------------------------
        // PHASE 1: Actor Validation
        // WHY: Extract head_id from actor field to ensure only head agents can
        // access STM. This prevents impersonation (actor is kernel-set).
        // -------------------------------------------------------------------------
        let head_id = extract_head_id(ctx)?;

        // -------------------------------------------------------------------------
        // PHASE 2: File Read with Legacy Migration
        // WHY: Modern STM is file-based, but we support transparent migration from
        // legacy SQLite storage on first read. This allows gradual migration without
        // requiring manual data export.
        // -------------------------------------------------------------------------
        let path = crate::runtime::workspace_head_memory(&ctx.cwd, &head_id);
        let stm = match crate::runtime::read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // WHY: One-time migration from legacy DB location. Attempt to load
                // from SQLite and migrate to file-based storage if found.
                let Some(k) = Kernel::get() else {
                    return Err(KernelError::internal("kernel not initialized"));
                };
                let Some(store) = k.store() else {
                    return Err(KernelError::internal("kernel store not attached"));
                };
                let legacy = store.get_head_stm(&head_id).unwrap_or_default();
                if !legacy.trim().is_empty() {
                    // WHY: Migrate to file-based storage with atomic write for safety.
                    // Ignore write failures (migration is best-effort).
                    let _ = crate::runtime::atomic_write_file_0600(&path, legacy.trim());
                    legacy
                } else {
                    String::new()
                }
            }
            // WHY: Return empty string on read errors (fail-safe default).
            Err(_) => String::new(),
        };

        // -------------------------------------------------------------------------
        // PHASE 3: Response Emission
        // WHY: Return full STM string plus metadata (head_id, length) for client use.
        // -------------------------------------------------------------------------
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "head_id": head_id,
                    "stm": stm,
                    "len": stm.len()
                }),
            ))
            .await;

        Ok(())
    }
}

// =============================================================================
// HELPERS
// =============================================================================

/// Extract head agent ID from syscall context actor field.
///
/// WHY: Centralizes actor validation to ensure only head agents can access STM.
/// Used by both `stm:read` and `stm:update` to enforce consistent access control.
///
/// SECURITY: Actor field is kernel-set (not caller-provided), preventing impersonation.
pub(crate) fn extract_head_id(ctx: &SyscallContext) -> Result<String, KernelError> {
    let actor = ctx
        .actor
        .as_deref()
        .unwrap_or("");
    if let Some(id) = actor.strip_prefix("head/") {
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    Err(KernelError::invalid_args("actor must be head/<id>"))
}
