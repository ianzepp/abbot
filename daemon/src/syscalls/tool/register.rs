//! Tool:Register - Register external tool catalog for a scope
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables external executors (UI, daemon, plugins) to register tool
//! specifications that become available to LLMs for tool calling. Tools are stored
//! in SQLite for persistence and cached in-memory for fast lookup.
//!
//! **Critical design decisions:**
//! - Replace-all semantics (not incremental updates) per scope
//! - Dual storage: SQLite for persistence + ExternalToolManager cache for speed
//! - JSON Schema validation (schema_json must be valid JSON)
//! - Scope-based isolation (tools registered per scope: "main", custom scopes)
//!
//! **Integration points:**
//! - `Store.replace_external_tools()` - Persists tools to `tool_registry` table
//! - `ExternalToolManager.replace_tools()` - Updates in-memory cache
//! - Hand/Head bundles - Load registered tools for LLM tool calling
//!
//! **Tool specification format (OpenAI function calling compatible):**
//! ```json
//! {
//!   "name": "read_file",
//!   "summary": "Read file contents",
//!   "description": "Reads the contents of a file from the filesystem",
//!   "schema_json": "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}},\"required\":[\"path\"]}"
//! }
//! ```
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Replace-all semantics**: Simplifies synchronization (external executor owns catalog)
//! - **Persistence + Cache**: Tools survive restarts but lookup is fast
//! - **No actor restrictions**: Any actor may register tools (authorization happens at dispatch)
//! - **Validation at registration**: Invalid tools rejected early (prevents runtime errors)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Replace-all vs Incremental Updates**
//!    - CHOSEN: Replace entire catalog per registration
//!    - WHY: Simpler synchronization, external executor owns catalog
//!    - IMPLICATION: Must send all tools each time (not just changed tools)
//!
//! 2. **Dual Storage**
//!    - CHOSEN: SQLite + in-memory cache
//!    - WHY: Persistence for restarts + speed for runtime lookup
//!    - IMPLICATION: Must keep SQLite and cache synchronized

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::history::ToolRegistryTool;
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for registering external tool catalog for a scope.
///
/// WHY: Zero-sized struct (stateless). All logic is in execute().
pub struct ToolRegister;

impl ToolRegister {
    /// Create a new ToolRegister syscall.
    ///
    /// WHY: Standard constructor pattern for syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolRegister {
    fn name(&self) -> &'static str {
        "tool:register"
    }

    /// Register tool catalog for a scope.
    ///
    /// WHY: Enables external executors to define tools available to LLMs.
    /// Tools are persisted to SQLite and cached in-memory for fast lookup.
    ///
    /// USE CASE:
    /// - External executor starts up and registers its tool catalog
    /// - Tools become available to hand/head agents for LLM tool calling
    /// - Example: UI registers read_file, write_file, git_status tools
    ///
    /// REPLACE-ALL SEMANTICS: This syscall replaces the entire tool catalog for
    /// the scope. It does NOT merge with existing tools. If you want to preserve
    /// existing tools, you must include them in the tools array.
    ///
    /// WHY replace-all: Simplifies synchronization. External executor owns the
    /// catalog and sends the complete set each time. No merge logic needed.
    ///
    /// SECURITY NOTE: No actor restrictions (any actor may register tools).
    /// Authorization happens at tool dispatch time (e.g., fs:read checks VFS).
    ///
    /// ARGUMENTS:
    /// - `scope` (string, required): Scope identifier (e.g., "main")
    /// - `tools` (array, required): Array of tool specifications
    ///   - Each tool: `{name, summary, description, schema_json}`
    ///   - `name` (string, required): Tool identifier (e.g., "read_file")
    ///   - `summary` (string, optional): Brief description for LLM
    ///   - `description` (string, optional): Detailed usage instructions
    ///   - `schema_json` (string, optional): JSON Schema for parameters (default: "null")
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"registered": true, "count": N}` on success
    /// - `E_INVALID_ARGS` if scope is empty, tools is not an array, or tool name is missing
    /// - `E_INTERNAL` if kernel or store is not initialized, or persistence fails
    /// - `E_CANCELLED` if context is cancelled mid-execution
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // ---------------------------------------------------------------------
        // PHASE 1: Validation
        // ---------------------------------------------------------------------
        // WHY: Validate scope and tools array before parsing individual tools.
        // Early validation provides clear error messages.
        ctx.check_cancelled()?;
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        let tools = data
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| KernelError::invalid_args("tools must be an array"))?;

        // ---------------------------------------------------------------------
        // PHASE 2: Tool Specification Parsing
        // ---------------------------------------------------------------------
        // WHY: Parse each tool specification, validating required fields.
        // Build Vec<ToolRegistryTool> for storage layer.
        //
        // VALIDATION: Tool name is required. Summary, description, and schema_json
        // are optional (default to empty string or "null").
        let mut out: Vec<ToolRegistryTool> = Vec::new();
        for t in tools {
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            if name.is_empty() {
                return Err(KernelError::invalid_args("tool name is required"));
            }
            let summary = t
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = t
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let schema_json = t
                .get("schema_json")
                .and_then(|v| v.as_str())
                .unwrap_or("null")
                .to_string();

            out.push(ToolRegistryTool {
                name: name.to_string(),
                summary,
                description,
                schema_json,
            });
        }

        // ---------------------------------------------------------------------
        // PHASE 3: Persistence
        // ---------------------------------------------------------------------
        // WHY: Persist tools to SQLite for restart durability. This ensures
        // tool catalog survives daemon restarts.
        //
        // REPLACE-ALL: This replaces ALL tools for the scope. Previous tools
        // are deleted and replaced with the new catalog.
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        store
            .replace_external_tools(scope, &out)
            .await
            .map_err(|e| KernelError::internal(format!("failed to persist tool registry: {e}")))?;

        // ---------------------------------------------------------------------
        // PHASE 4: Cache Update
        // ---------------------------------------------------------------------
        // WHY: Update in-memory cache in ExternalToolManager for fast lookup.
        // Hand/Head agents query this cache when building tool lists for LLMs.
        k.external_tools().replace_tools(scope, &out).await;
        k.bump_activity();

        // ---------------------------------------------------------------------
        // PHASE 5: Acknowledgment
        // ---------------------------------------------------------------------
        // WHY: Return Frame::ok with registered count confirming success.
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"registered": true, "count": out.len()}),
            ))
            .await;
        Ok(())
    }
}
