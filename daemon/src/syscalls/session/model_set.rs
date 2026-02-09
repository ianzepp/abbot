//! Session:Model_Set - Runtime LLM model switching for session scopes
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables runtime switching of LLM models for specific session scopes
//! (e.g., "main", "session/<id>"). Model preferences are persisted in SQLite via the
//! kernel's history store and survive kernel restarts.
//!
//! **Critical integration points:**
//! - `Store::set_session_model()` - Persists model choice to `session_model` table (SQLite)
//! - `Kernel::get()` - Accesses global kernel instance for store retrieval
//! - `SyscallContext::require_mutation()` - Enforces "head" actor authorization
//!
//! **Persistence strategy:**
//! - Database: `history.db` (kernel store)
//! - Table: `session_model` (columns: scope TEXT PRIMARY KEY, model TEXT, updated_at INTEGER)
//! - Upsert semantics: `INSERT ... ON CONFLICT(scope) DO UPDATE SET model = ?`
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{scope, model, reset}` on successful update
//! - Returns `KernelError` for invalid arguments, missing kernel, or store errors
//!
//! SECURITY MODEL
//! ==============
//! This syscall modifies persistent session state, requiring strict authorization:
//!
//! 1. **Actor Authorization**
//!    - WHY: Model switching affects LLM behavior system-wide for a session scope
//!    - HOW: `ctx.require_mutation()` enforces "head" actor requirement (line 88)
//!    - ATTACK PREVENTED: "Hand" agents (controlled by LLMs) cannot switch themselves
//!      to more powerful or expensive models, preventing cost exploitation
//!
//! 2. **Scope Isolation**
//!    - WHY: Each session scope has independent model preference (default: "main")
//!    - HOW: Scope is stored as SQLite PRIMARY KEY in `session_model` table
//!    - IMPLICATION: Sessions cannot interfere with each other's model choices
//!
//! 3. **Validation Strategy**
//!    - CHOSEN: Minimal validation (only trim + empty check on model string)
//!    - WHY: Model names are provider-dependent (e.g., "claude-3-5-sonnet-20241022",
//!      "gpt-4-turbo", "anthropic.claude-v2") and cannot be validated at syscall time
//!    - TRADE-OFF: Invalid model names are accepted by the syscall but will fail at
//!      LLM invocation time with provider-specific errors
//!    - ACCEPTABLE BECAUSE: Provider APIs reject invalid models, no security risk
//!
//! 4. **No Model Allowlist**
//!    - REJECTED: Restricting model names to hardcoded allowlist
//!    - WHY: Model catalog changes frequently (new releases, deprecations, regional variants)
//!    - IMPLICATION: Any string can be set as model name (fails later if invalid)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Deferred validation**: Validate model names at LLM runtime, not syscall time
//! - **Session-scoped state**: Model preference persists per session scope
//! - **Mutation guard required**: Only authorized "head" agents may switch models
//! - **Upsert semantics**: Updating existing scope overwrites previous model choice
//! - **Explicit scope control**: Defaults to "main" but allows custom session scopes
//!
//! TRADE-OFFS
//! ==========
//! 1. **No Model Name Validation**
//!    - CHOSEN: Accept any non-empty string as model name
//!    - REJECTED: Validate against provider model catalog
//!    - WHY: Model names vary by provider (Anthropic, OpenAI, AWS Bedrock, Azure)
//!      and new models are released frequently (would require constant updates)
//!    - IMPLICATION: Typos in model names are accepted by syscall, fail at runtime
//!    - ACCEPTABLE: Provider APIs return clear error messages for invalid models
//!
//! 2. **Reset Flag Unused**
//!    - CURRENT: `reset` argument is accepted but not acted upon
//!    - WHY: Originally intended for resetting conversation context, but model
//!      switching alone doesn't clear conversation history (separate concern)
//!    - IMPLICATION: `reset: true` has no effect, included in response for API compat
//!
//! 3. **Synchronous Store Access**
//!    - CHOSEN: Direct SQLite write via `store.set_session_model()` (blocking)
//!    - WHY: Model switching is infrequent (user-initiated) and fast (single row upsert)
//!    - IMPLICATION: Brief lock contention on store mutex during write
//!    - ACCEPTABLE: Sub-millisecond operation, no observable latency

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `session:model_set` syscall.
///
/// WHY: Structured model switching specification with scope isolation.
#[derive(Debug, Deserialize)]
struct SessionModelSetArgs {
    /// Model identifier (provider-specific format).
    ///
    /// WHY: Validated only for non-emptiness. Provider APIs reject invalid names.
    /// Examples: "claude-3-5-sonnet-20241022", "gpt-4-turbo", "anthropic.claude-v2"
    model: String,

    /// Session scope for model preference (default: "main").
    ///
    /// WHY: Enables independent model choices per session (e.g., "main", "session/123").
    /// Defaults to "main" scope if unspecified.
    #[serde(default)]
    room: Option<String>,

    /// Whether to reset conversation context (currently unused).
    ///
    /// WHY: Included for API compatibility but not implemented. Model switching
    /// alone doesn't clear conversation history (separate concern handled elsewhere).
    #[serde(default)]
    reset: Option<bool>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for switching LLM models at runtime for session scopes.
///
/// WHY: Zero-sized type - no state needed, delegates to kernel store for persistence.
pub struct SessionModelSet;

impl Default for SessionModelSet {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionModelSet {
    /// Create a new `SessionModelSet` syscall.
    ///
    /// WHY: Standard constructor for stateless syscall (no configuration needed).
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for SessionModelSet {
    fn name(&self) -> &'static str {
        "session:model_set"
    }

    /// Switch LLM model for a session scope with persistent storage.
    ///
    /// WHY: Enables dynamic model switching without restarting the kernel. Common
    /// use cases include:
    /// - Testing with cheaper models before production runs
    /// - Switching to more powerful models for complex tasks
    /// - A/B testing different model versions
    /// - Regional model selection (e.g., EU vs. US endpoints)
    ///
    /// USE CASE: Invoked by "head" agents (user commands, system orchestration) to
    /// change which LLM model is used for subsequent chat/llm syscalls in the same scope.
    /// "Hand" and "room" agents cannot invoke this syscall (mutation guard prevents it).
    ///
    /// SECURITY NOTE: This syscall modifies persistent session state, requiring:
    /// 1. Actor verification - only "head" agents may switch models (line 88)
    /// 2. Scope isolation - model preferences are per-scope (no cross-session interference)
    /// 3. Minimal validation - model names are not validated at syscall time (deferred to LLM runtime)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{scope, model, reset}` on successful model switch
    /// - `E_FORBIDDEN` if actor lacks mutation permission
    /// - `E_INTERNAL` if kernel not initialized or store not attached
    /// - `E_INVALID_ARGS` if model name is empty or arguments malformed
    /// - `E_IO` if SQLite write fails
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Security Verification & Kernel Access
        // =====================================================================
        // WHY: Verify cancellation and authorization before accessing kernel state.
        ctx.check_cancelled()?;

        // WHY: Only "head" agents may switch models. This prevents "hand" agents
        // (which execute LLM-generated tool calls) from escalating to more powerful
        // or expensive models if the LLM is compromised or misbehaves.
        //
        // ATTACK SCENARIO: Without mutation guard, malicious LLM could switch itself
        // from "claude-3-haiku" (cheap) to "claude-opus-4" (expensive) to drain budget.
        ctx.require_mutation()?;

        // WHY: Access kernel store for persistent model preference. Store is global
        // (one SQLite database per kernel instance) and shared across all sessions.
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        // =====================================================================
        // PHASE 2: Argument Parsing & Validation
        // =====================================================================
        let args: SessionModelSetArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Trim whitespace from model name (common user input error).
        // Empty check is the ONLY validation - model names are provider-specific
        // and cannot be validated without querying provider APIs.
        let model = args.model.trim();
        if model.is_empty() {
            return Err(KernelError::invalid_args("model is empty"));
        }

        // WHY: Default to "main" scope if unspecified. "main" is the primary session
        // scope for single-user CLI usage. Multi-user servers use "session/<id>".
        let room = args.room.as_deref().unwrap_or("main");

        // =====================================================================
        // PHASE 3: Persistent Model Update
        // =====================================================================
        // WHY: Upsert model preference to SQLite. Store method handles:
        // - Trimming scope/model strings
        // - Generating updated_at timestamp
        // - INSERT ... ON CONFLICT(scope) DO UPDATE SET model = ? (atomic upsert)
        //
        // CONCURRENCY: Store uses mutex-guarded SQLite connection. Brief lock
        // contention possible if multiple sessions update models simultaneously,
        // but operation is fast (single row upsert, sub-millisecond).
        store
            .set_room_model(room, model)
            .await
            .map_err(|e| KernelError::io(format!("failed to set session model: {e}")))?;

        // =====================================================================
        // PHASE 4: Response Formatting
        // =====================================================================
        // WHY: Echo back scope, model, and reset flag for confirmation. Clients
        // can verify the model switch succeeded by comparing request vs. response.
        //
        // NOTE: `reset` flag is included in response for API compatibility but has
        // no effect (model switching alone doesn't clear conversation history).
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "room": room,
                    "model": model,
                    "reset": args.reset.unwrap_or(false)
                }),
            ))
            .await;

        Ok(())
    }
}
