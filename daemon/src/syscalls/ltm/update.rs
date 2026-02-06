//! LTM:Update - Modify long-term memory with structured operations
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides controlled mutation of agent long-term memory through
//! structured append/replace/remove operations. It implements a read-modify-write
//! pattern with atomic filesystem updates to prevent corruption.
//!
//! **Storage strategy:**
//! - Primary: Markdown file at `.abbot/workspace/mind/memory.md`
//! - Legacy: SQLite `head_memory` table (one-time migration on first access)
//! - Atomic writes: 0600 permissions to prevent unauthorized access
//!
//! **Operation types:**
//! - `append`: Add content to end of LTM (with double-newline separator)
//! - `replace`: Find pattern and replace with new content
//! - `remove`: Delete pattern and clean up excessive whitespace
//!
//! **Integration with kernel:**
//! - Requires mutation permission (only "head" agents may update LTM)
//! - Respects cancellation tokens (early exit if task cancelled)
//! - Validates kernel and store initialization before proceeding
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Structured operations over free edits**: Prevents corruption from arbitrary writes
//! - **Read-current-state first**: Ensures updates apply to latest content
//! - **One-time migration**: Automatically imports legacy DB LTM on first access
//! - **Atomic writes**: Filesystem operations are atomic to prevent partial updates
//! - **Whitespace normalization**: Removes excessive blank lines after removals
//!
//! CONCURRENCY
//! ===========
//! - Read-modify-write pattern is safe because:
//!   1. Only "head" agents can write (single writer per workspace)
//!   2. Filesystem atomic write (no torn writes)
//!   3. No concurrent modification expected in typical usage
//!
//! TRADE-OFF: Does not implement file locking or versioning. This is acceptable
//! because LTM updates are rare and typically initiated by single agent.
//!
//! SECURITY MODEL
//! ==============
//! - Requires `ctx.require_mutation()` - only "head" agents may update LTM
//! - WHY: Prevents compromised "hand" agents (executing tool calls from LLM)
//!   from injecting false context into long-term memory
//! - File permissions: 0600 (owner read/write only)
//! - WHY: Prevents unauthorized processes from reading agent memory
//!
//! LEGACY MIGRATION
//! ================
//! On first access, checks for legacy LTM in SQLite `head_memory` table.
//! If found, migrates content to filesystem and uses that going forward.
//!
//! WHY: Older Abbot versions stored LTM in database. File-based storage is
//! preferred for human readability and git-committability.
//!
//! MEMORY OPERATIONS
//! =================
//! **Append Operation:**
//! - Adds content to end of LTM with double-newline separator
//! - Skips empty content (no-op)
//! - Use case: Recording new decisions, facts, or preferences
//!
//! **Replace Operation:**
//! - Finds first occurrence of pattern and replaces with new content
//! - Skips if pattern not found (no-op, not an error)
//! - Use case: Updating outdated information or correcting mistakes
//!
//! **Remove Operation:**
//! - Deletes all occurrences of pattern
//! - Normalizes whitespace (removes triple newlines, trims excess)
//! - Skips if pattern not found (no-op)
//! - Use case: Removing obsolete or incorrect information
//!
//! PERFORMANCE
//! ===========
//! - LTM is typically small (few KB), so in-memory operations are fast
//! - Atomic write prevents need for fsync on every update
//! - Legacy migration happens once per workspace lifetime
//!
//! TRADE-OFFS
//! ==========
//! 1. **String Operations vs. Structured Format**
//!    - CHOSEN: Plain text string operations (find/replace)
//!    - REJECTED: Structured format like JSON or YAML
//!    - WHY: Markdown is human-readable and LLM-friendly
//!    - IMPLICATION: Pattern matching can be fragile if content changes
//!
//! 2. **No Version History**
//!    - CHOSEN: Single file, overwrite on update
//!    - WHY: Simplicity, rely on git for version control if needed
//!    - IMPLICATION: No built-in rollback or audit trail
//!
//! 3. **No File Locking**
//!    - CHOSEN: Atomic write without explicit locks
//!    - WHY: Single writer assumption (only one head agent per workspace)
//!    - IMPLICATION: Concurrent updates from multiple processes would conflict

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `ltm:update` syscall.
///
/// WHY: Batched operations allow multiple LTM changes in a single atomic
/// filesystem write, improving performance and consistency.
#[derive(Debug, Deserialize)]
struct LtmUpdateArgs {
    /// List of LTM operations to apply in sequence.
    ///
    /// WHY: Operations are applied in order, allowing complex updates like
    /// "remove old fact, then append corrected fact" in single syscall.
    ops: Vec<LtmOp>,
}

/// Single LTM operation (append, replace, or remove).
///
/// WHY: Structured operations prevent arbitrary edits that could corrupt LTM.
/// Each operation has clear semantics and validation rules.
#[derive(Debug, Deserialize)]
struct LtmOp {
    /// Operation type: "append", "replace", or "remove".
    ///
    /// WHY: String-based for JSON compatibility with LLM tool calls.
    kind: String,

    /// Content for append/replace operations.
    ///
    /// WHY: Optional (defaults to empty) because remove operations don't need content.
    #[serde(default)]
    content: String,

    /// Pattern for replace/remove operations.
    ///
    /// WHY: Optional (defaults to empty) because append operations don't need pattern.
    #[serde(default)]
    pattern: String,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for modifying agent long-term memory.
///
/// WHY: Stateless design allows easy instantiation and testing. All state
/// (LTM content, workspace path) comes from syscall context and kernel.
pub struct LtmUpdate;

impl LtmUpdate {
    /// Create a new `LtmUpdate` syscall.
    ///
    /// WHY: Simple constructor with no dependencies, making testing straightforward.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LtmUpdate {
    fn name(&self) -> &'static str {
        "ltm:update"
    }

    /// Modify long-term memory with structured operations.
    ///
    /// WHY: Provides controlled LTM mutation through append/replace/remove operations,
    /// preventing arbitrary edits that could corrupt agent memory.
    ///
    /// USE CASE: Invoked by "head" agents to update persistent memory:
    /// - Record important decisions ("ALWAYS use X pattern for Y")
    /// - Update outdated information (replace old fact with corrected fact)
    /// - Remove obsolete context (delete information no longer relevant)
    ///
    /// SECURITY NOTE: Requires mutation permission (only "head" agents allowed).
    /// This prevents compromised "hand" agents from injecting false context.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{applied: [...operations], ltm_len: bytes}`
    /// - `E_FORBIDDEN` if actor lacks mutation permission
    /// - `E_INTERNAL` if kernel or store not initialized
    /// - `E_INVALID_ARGS` if arguments are malformed
    /// - `E_IO` if filesystem write fails
    /// - `E_CANCELLED` if parent context cancels during execution
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // ---------------------------------------------------------------------
        // PHASE 1: Security Verification
        // ---------------------------------------------------------------------
        // WHY: Check cancellation and authorization before expensive operations.
        // Early exit prevents wasted work on already-cancelled tasks.
        ctx.check_cancelled()?;

        // WHY: Only "head" agents may modify LTM. This prevents "hand" agents
        // (which execute tool calls from potentially compromised LLMs) from
        // injecting false context into long-term memory.
        ctx.require_mutation()?;

        // ---------------------------------------------------------------------
        // PHASE 2: Kernel Initialization Check
        // ---------------------------------------------------------------------
        // WHY: Kernel and store must be initialized before accessing LTM.
        // Early validation provides clear error messages if misconfigured.
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        // ---------------------------------------------------------------------
        // PHASE 3: Argument Parsing & Validation
        // ---------------------------------------------------------------------
        // WHY: Validate arguments before filesystem operations to provide
        // clear error messages for malformed requests.
        let args: LtmUpdateArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // ---------------------------------------------------------------------
        // PHASE 4: Load Current LTM (with Legacy Migration)
        // ---------------------------------------------------------------------
        // WHY: Read-modify-write pattern ensures updates apply to latest content.
        // One-time migration imports legacy database LTM on first access.
        //
        // TRADE-OFF: No file locking. Acceptable because only one head agent
        // per workspace, and concurrent updates are rare.
        let path = crate::runtime::workspace_mind_memory(&ctx.cwd);
        let current = match crate::runtime::read_optional_file(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                // WHY: One-time migration from legacy DB storage to filesystem.
                // Older Abbot versions stored LTM in SQLite. File-based storage
                // is preferred for human readability and git-committability.
                let legacy = store.get_head_ltm("conclave").await.unwrap_or_default();
                if !legacy.trim().is_empty() {
                    // WHY: Write legacy content to filesystem, so subsequent
                    // accesses use file-based storage instead of database.
                    let _ = crate::runtime::atomic_write_file_0600(&path, legacy.trim());
                    legacy
                } else {
                    String::new()
                }
            }
            Err(_) => String::new(),
        };

        // ---------------------------------------------------------------------
        // PHASE 5: Apply Operations in Sequence
        // ---------------------------------------------------------------------
        // WHY: Operations are applied in order, allowing complex updates like
        // "remove old fact, then append corrected fact" in single syscall.
        //
        // PERFORMANCE: All operations are in-memory string manipulations, fast
        // even for large LTM (typically only a few KB).
        let mut ltm = current.clone();
        let mut applied = Vec::new();

        for op in args.ops {
            match op.kind.as_str() {
                // WHY: Append adds content to end with double-newline separator.
                // This maintains readable paragraph structure in markdown.
                "append" => {
                    let content = op.content.trim();
                    if content.is_empty() {
                        continue; // WHY: Skip no-op operations
                    }
                    if !ltm.is_empty() {
                        ltm.push_str("\n\n"); // WHY: Paragraph separator
                    }
                    ltm.push_str(content);
                    applied.push(json!({"kind": "append"}));
                }
                // WHY: Replace finds first occurrence and substitutes content.
                // Use case: Updating outdated information or correcting mistakes.
                "replace" => {
                    let pattern = op.pattern;
                    if pattern.is_empty() {
                        continue; // WHY: Skip no-op operations
                    }
                    if let Some(pos) = ltm.find(&pattern) {
                        let end = pos + pattern.len();
                        let replacement = op.content;
                        ltm.replace_range(pos..end, &replacement);
                        applied.push(json!({"kind": "replace", "pattern": pattern}));
                    }
                    // WHY: Pattern not found is not an error - allows idempotent ops
                }
                // WHY: Remove deletes pattern and normalizes whitespace.
                // Use case: Removing obsolete or incorrect information.
                "remove" => {
                    let pattern = op.pattern;
                    if pattern.is_empty() {
                        continue; // WHY: Skip no-op operations
                    }
                    if ltm.contains(&pattern) {
                        ltm = ltm.replace(&pattern, "");
                        // WHY: Normalize excessive blank lines after removal.
                        // Prevents LTM from accumulating whitespace over time.
                        while ltm.contains("\n\n\n") {
                            ltm = ltm.replace("\n\n\n", "\n\n");
                        }
                        ltm = ltm.trim().to_string();
                        applied.push(json!({"kind": "remove", "pattern": pattern}));
                    }
                    // WHY: Pattern not found is not an error - allows idempotent ops
                }
                _ => {
                    // WHY: Unknown operation types are silently ignored to allow
                    // forward compatibility (older kernels ignore new op types).
                }
            }
        }

        // ---------------------------------------------------------------------
        // PHASE 6: Atomic Filesystem Write
        // ---------------------------------------------------------------------
        // WHY: Only write if content changed, avoiding unnecessary filesystem
        // operations and preserving file modification time.
        if ltm != current {
            // WHY: Atomic write prevents torn writes if process crashes mid-write.
            // 0600 permissions prevent unauthorized processes from reading LTM.
            if let Err(e) = crate::runtime::atomic_write_file_0600(&path, &ltm) {
                return Err(KernelError::io(format!("failed to save LTM file: {e}")));
            }
        }

        // ---------------------------------------------------------------------
        // PHASE 7: Response
        // ---------------------------------------------------------------------
        // WHY: Return applied operations so callers know which succeeded.
        // Include final LTM length for debugging and monitoring.
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"applied": applied, "ltm_len": ltm.len()}),
            ))
            .await;

        Ok(())
    }
}
