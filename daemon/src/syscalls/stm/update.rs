//! STM:Update - Modify short-term memory for a head agent
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall modifies the Short-Term Memory (STM) for a head agent via three
//! operations: `set` (replace), `append` (add to end), and `clear` (delete).
//! STM is stored in workspace-local files with atomic writes for safety.
//!
//! **Operations:**
//! - `set`: Replace entire STM with new content (overwrites previous)
//! - `append`: Add content to end with double-newline separator
//! - `clear`: Delete all STM content (empty string)
//!
//! **Storage location:**
//! - Modern: `.abbot/memory/head-<id>.txt` (workspace-local file)
//! - Legacy: SQLite `head_stm` table (migrated on first update)
//!
//! **Persistence strategy:**
//! Uses `atomic_write_file_0600()` for crash-safe writes (write to temp file,
//! then atomic rename). File permissions are set to 0600 (owner-only) for security.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Atomic writes**: Prevent partial writes from leaving STM in corrupt state
//! - **Mutation guard**: Requires "head" actor + mutation permission (double validation)
//! - **Legacy migration**: Transparently migrates SQLite-based STM on first update
//! - **Fail-fast on I/O errors**: Unlike read (which returns empty string), update
//!   returns error on write failures to prevent silent data loss
//!
//! CONCURRENCY
//! ===========
//! - Atomic writes prevent partial updates from concurrent modifications
//! - No locking required (each head agent has isolated STM file)
//! - TRADE-OFF: Last-write-wins if concurrent updates to same head's STM
//!   (acceptable because head agents run sequentially per task lane)
//!
//! SECURITY MODEL
//! ==============
//! - **Actor validation**: Must be `head/<id>` format (prevents impersonation)
//! - **Mutation permission**: Requires `ctx.require_mutation()` (head-only)
//! - **File permissions**: STM files are 0600 (owner-only read/write)
//! - **No cross-agent access**: Each head can only modify its own STM
//!
//! TRADE-OFFS
//! ==========
//! 1. **Atomic writes vs. append-only**
//!    - CHOSEN: Atomic writes with full file replacement
//!    - WHY: STM is typically small (< 10KB), so replacement is fast
//!    - COST: Entire file rewritten on each update (acceptable for small files)
//!
//! 2. **Set/append/clear ops vs. text editing**
//!    - CHOSEN: Three simple operations (no line-level editing)
//!    - WHY: STM is unstructured text, not a structured document
//!    - COST: No granular editing (must read, modify, set for complex changes)
//!
//! 3. **Append separator**
//!    - CHOSEN: Double-newline (`\n\n`) between appended content
//!    - WHY: Provides visual separation for multi-append STM
//!    - COST: May introduce extra whitespace (callers can use set for exact control)
//!
//! WHO CAN USE
//! ===========
//! - Only head agents (`head/<id>` actor format + mutation permission)
//! - Each agent can only modify its own STM (no cross-agent access)

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `stm:update` syscall.
///
/// WHY: Structured arguments for operation type (set/append/clear) and content.
#[derive(Debug, Deserialize)]
struct StmUpdateArgs {
    /// Operation type: "set", "append", or "clear".
    ///
    /// WHY: Explicit operation type prevents ambiguity (e.g., empty content with
    /// set vs. clear) and enables validation.
    op: String,

    /// Content for set/append operations.
    ///
    /// WHY: Defaults to empty string for clear operation (where content is ignored).
    #[serde(default)]
    content: String,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for modifying short-term memory for a head agent.
///
/// WHY: Enables head agents to update session-scoped context via set/append/clear
/// operations without requiring direct file system access.
pub struct StmUpdate;

impl StmUpdate {
    /// Create a new `StmUpdate` syscall.
    ///
    /// WHY: Zero-state syscall (update operations are stateless).
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for StmUpdate {
    fn name(&self) -> &'static str {
        "stm:update"
    }

    /// Modify the short-term memory for the calling head agent.
    ///
    /// WHY: Provides mutable session-scoped context for agents to track progress,
    /// remember decisions, or cache lookups across task executions.
    ///
    /// ARGUMENTS:
    /// - `op`: "set" (replace), "append" (add to end), or "clear" (delete)
    /// - `content`: New content for set/append (ignored for clear)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{head_id, op, stm_len}` (new STM length after update)
    ///
    /// SECURITY NOTE: Requires head actor (`head/<id>`) + mutation permission.
    /// This double-validation prevents both impersonation and unauthorized mutation.
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // -------------------------------------------------------------------------
        // PHASE 1: Permission & Actor Validation
        // WHY: Require mutation permission (head-only) and validate actor format.
        // This prevents "hand" agents from modifying STM even if they impersonate
        // a head actor (actor field is kernel-set, not caller-provided).
        // -------------------------------------------------------------------------
        ctx.require_mutation()?;
        let head_id = super::read::extract_head_id(ctx)?;

        // -------------------------------------------------------------------------
        // PHASE 2: Argument Parsing
        // WHY: Parse operation type and content before reading current STM to
        // fail fast on invalid arguments.
        // -------------------------------------------------------------------------
        let args: StmUpdateArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // -------------------------------------------------------------------------
        // PHASE 3: Current STM Read with Legacy Migration
        // WHY: Load current STM for append operation and migrate legacy SQLite-based
        // STM on first update (same as stm:read migration logic).
        // -------------------------------------------------------------------------
        let path = crate::runtime::workspace_head_memory(&ctx.cwd, &head_id);
        let current = match crate::runtime::read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // WHY: One-time migration from legacy DB location (same as stm:read).
                let Some(k) = Kernel::get() else {
                    return Err(KernelError::internal("kernel not initialized"));
                };
                let Some(store) = k.store() else {
                    return Err(KernelError::internal("kernel store not attached"));
                };
                let legacy = store.get_head_stm(&head_id).await.unwrap_or_default();
                if !legacy.trim().is_empty() {
                    // WHY: Migrate to file-based storage (ignore write failures for migration).
                    let _ = crate::runtime::atomic_write_file_0600(&path, legacy.trim());
                    legacy
                } else {
                    String::new()
                }
            }
            // WHY: Default to empty string on read errors (fail-safe).
            Err(_) => String::new(),
        };

        // -------------------------------------------------------------------------
        // PHASE 4: STM Transformation
        // WHY: Apply the requested operation (set/append/clear) to current STM.
        // Append uses double-newline separator for visual clarity.
        // -------------------------------------------------------------------------
        let new_stm = match args.op.as_str() {
            // WHY: Set operation replaces entire STM (overwrites previous content).
            "set" => args.content.clone(),

            // WHY: Append operation adds content to end with double-newline separator.
            // If either current or new content is empty, skip separator to avoid extra whitespace.
            "append" => {
                if current.is_empty() {
                    args.content.clone()
                } else if args.content.is_empty() {
                    current
                } else {
                    format!("{}\n\n{}", current, args.content)
                }
            }

            // WHY: Clear operation deletes all STM content (returns empty string).
            "clear" => String::new(),

            // WHY: Reject unknown operations to prevent silent no-ops on typos.
            _ => {
                return Err(KernelError::invalid_args(format!(
                    "unknown op: {}",
                    args.op
                )))
            }
        };

        // -------------------------------------------------------------------------
        // PHASE 5: Atomic Write to Disk
        // WHY: Write new STM to disk with atomic file operation (write to temp,
        // then rename). File permissions are 0600 (owner-only) for security.
        // -------------------------------------------------------------------------
        if let Err(e) = crate::runtime::atomic_write_file_0600(&path, &new_stm) {
            return Err(KernelError::io(format!("failed to save STM file: {e}")));
        }

        // -------------------------------------------------------------------------
        // PHASE 6: Response Emission
        // WHY: Return confirmation with head_id, operation, and new STM length.
        // -------------------------------------------------------------------------
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "head_id": head_id,
                    "op": args.op,
                    "stm_len": new_stm.len()
                }),
            ))
            .await;

        Ok(())
    }
}
