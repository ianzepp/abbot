//! LLM:Chaos:List - Query available chaos axes and trait levels
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides introspection into the chaos engineering framework by
//! listing all available trait axes and their possible values. It enables:
//!
//! - **Test discovery**: Find out which behavioral variations are available
//! - **Pinning validation**: Ensure pinned trait names are valid before testing
//! - **UI generation**: Build dynamic test configuration interfaces
//! - **Documentation**: Generate reference docs for chaos trait options
//!
//! **Integration points:**
//! - `runtime::chaos::list_axes()` for fetching axis definitions
//! - Test harnesses and UIs for chaos configuration
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with JSON object mapping axis names to trait arrays
//!   Example: `{"emotion": ["calm", "anxious_L1", ...], "bias": ["neutral", ...]}`
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Introspection-first**: Chaos framework is self-describing via syscall
//! - **No arguments**: Pure query operation, no parameters needed
//! - **Stable output**: Axis definitions are static, loaded at runtime initialization
//! - **JSON-friendly**: Output is directly consumable by test harnesses and UIs
//!
//! USE CASES
//! =========
//! 1. **Test configuration**: List available traits before generating test cases
//! 2. **Validation**: Ensure user-provided pinned traits exist before running tests
//! 3. **Documentation**: Auto-generate reference docs for chaos trait options
//! 4. **UI building**: Populate dropdowns/checkboxes for interactive chaos config
//! 5. **Debugging**: Verify chaos module loaded correctly (non-empty axes list)
//!
//! OUTPUT FORMAT
//! =============
//! WHY: JSON object with axis names as keys, trait arrays as values. This format
//! enables easy lookup (e.g., `result["emotion"]`) and iteration.
//!
//! EXAMPLE:
//! ```json
//! {
//!   "emotion": ["calm", "anxious_L1", "anxious_L2", "confident_L1", ...],
//!   "bias": ["neutral", "confirmation_bias_L1", "anchoring_L2", ...],
//!   "style": ["balanced", "verbose_L1", "terse_L1", ...],
//!   "error_tendency": ["accurate", "typos_L1", "tangents_L1", ...]
//! }
//! ```
//!
//! EACH ARRAY: Contains all possible trait values for that axis. Arrays include
//! both neutral/baseline traits ("calm", "neutral") and leveled variations
//! ("anxious_L1", "anxious_L2", ..., "anxious_L5").
//!
//! CONCURRENCY
//! ===========
//! - Chaos axis definitions are static (loaded once at runtime initialization)
//! - Query is read-only, no shared mutable state
//! - Safe for concurrent execution across multiple task lanes
//! - No actor restrictions (all agents may query chaos axes)
//!
//! PERFORMANCE
//! ===========
//! - Query is fast (<1ms) - just cloning static data structures
//! - No network I/O, no file access, no database queries
//! - Memory usage is minimal (single HashMap clone + JSON serialization)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Static vs. Dynamic Axes**
//!    - CHOSEN: Static axes loaded at runtime initialization
//!    - REJECTED: Dynamic axes loaded from config files or plugins
//!    - WHY: Simpler implementation, predictable behavior
//!    - IMPLICATION: Adding new axes requires code changes + recompilation
//!
//! 2. **Full List vs. Summary**
//!    - CHOSEN: Return full list of all traits for all axes
//!    - REJECTED: Return only axis names, require separate query per axis
//!    - WHY: Single query is more efficient for test configuration UIs
//!    - IMPLICATION: Slightly larger response payload (~1-2KB JSON)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for listing available chaos axes and trait levels.
///
/// WHY: Enables introspection of chaos framework for test configuration,
/// validation, and UI generation. Pure query operation with no side effects.
pub struct LlmChaosList;

impl Default for LlmChaosList {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmChaosList {
    /// Create a new `LlmChaosList` syscall.
    ///
    /// WHY: Standard constructor for syscall registration.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LlmChaosList {
    fn name(&self) -> &'static str {
        "llm:chaos:list"
    }

    /// List all available chaos axes and their trait levels.
    ///
    /// WHY: Enables test harnesses, UIs, and documentation generators to discover
    /// available chaos traits without hardcoding axis names or trait values.
    ///
    /// USE CASE: Invoked by:
    /// - Test configuration tools to populate trait selection dropdowns
    /// - Validation logic to ensure pinned traits exist before testing
    /// - Documentation generators to produce chaos trait reference docs
    /// - Debugging tools to verify chaos module loaded correctly
    ///
    /// ARGUMENTS: None (pure query operation)
    ///
    /// RETURNS:
    /// - `Frame::ok` with JSON object mapping axis names to trait arrays
    ///   Example: `{"emotion": ["calm", "anxious_L1", ...], "bias": ["neutral", ...]}`
    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // WHY: Delegate to chaos::list_axes() for axis definitions. This function
        // returns a HashMap<String, Vec<String>> with axis names as keys and
        // trait arrays as values.
        let axes = crate::runtime::chaos::list_axes();

        // WHY: Convert HashMap to serde_json::Map for JSON serialization. Each
        // axis name becomes a JSON object key, each trait array becomes a JSON array.
        // This format is directly consumable by test harnesses and UIs.
        let map: serde_json::Map<String, serde_json::Value> = axes
            .into_iter()
            .map(|(name, levels)| (name.to_string(), json!(levels)))
            .collect();

        // WHY: Return Frame::ok with JSON object. No pagination needed - full
        // axis list is small (~1-2KB JSON). Clients can filter/search locally.
        let _ = tx
            .send(Frame::ok(ctx.call_id, serde_json::Value::Object(map)))
            .await;

        Ok(())
    }
}
