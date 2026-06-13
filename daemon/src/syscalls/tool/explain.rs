//! Tool:Explain - Lookup tool specification by name
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall retrieves tool specifications from the tool registry for inspection
//! and debugging. It enables agents to discover tool parameters, descriptions, and
//! JSON schemas before invoking them.
//!
//! **Critical design decisions:**
//! - Lookup by name + room + source (not just name)
//! - Returns full specification including JSON schema
//! - user__ prefix stripping (backward compatibility with old tool naming)
//! - "external" source only (internal/plugin sources reserved for future use)
//!
//! **Integration points:**
//! - `Store.get_tool()` - Queries `tool_registry` table in SQLite
//! - Hand/Head agents - Query tool specs for debugging tool call failures
//! - UI - Display tool specifications to users
//!
//! **Use cases:**
//! - Agent inspects tool spec to understand parameters before calling
//! - User debugs tool call failure by examining expected schema
//! - UI displays available tools with descriptions
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Explicit source**: Require source parameter (future-proofs for internal/plugin tools)
//! - **Full specification**: Return complete tool spec (not just summary)
//! - **No actor restrictions**: Any actor may query tool specs (read-only operation)
//! - **Backward compatibility**: Strip user__ prefix for old tool naming convention
//!
//! TRADE-OFFS
//! ==========
//! 1. **Lookup by name (not prefix/glob)**
//!    - CHOSEN: Exact name match only
//!    - WHY: Simpler implementation, clearer semantics
//!    - IMPLICATION: Cannot list all tools starting with prefix (use separate syscall for discovery)
//!
//! 2. **Source parameter required**
//!    - CHOSEN: Require explicit source parameter
//!    - WHY: Future-proofs for internal/plugin sources (avoids ambiguity)
//!    - IMPLICATION: Callers must specify source (defaults to "external")

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for tool:explain syscall.
///
/// WHY: Structured arguments with defaults for room and source.
#[derive(Debug, Deserialize)]
struct ToolExplainArgs {
    /// Tool name to lookup.
    ///
    /// WHY: Required field. The tool identifier (e.g., "read_file").
    /// user__ prefix is stripped for backward compatibility.
    name: String,

    /// Room identifier.
    ///
    /// WHY: Optional, defaults to "main". Enables room-based tool isolation.
    #[serde(default)]
    room: Option<String>,

    /// Tool source.
    ///
    /// WHY: Optional, defaults to "external". Enables future internal/plugin sources.
    /// Currently only "external" is supported.
    #[serde(default)]
    source: Option<String>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for looking up tool specifications by name.
///
/// WHY: Zero-sized struct (stateless). All logic is in execute().
pub struct ToolExplain;

impl Default for ToolExplain {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExplain {
    /// Create a new ToolExplain syscall.
    ///
    /// WHY: Standard constructor pattern for syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ToolExplain {
    fn name(&self) -> &'static str {
        "tool:explain"
    }

    /// Lookup tool specification by name.
    ///
    /// WHY: Enables agents to inspect tool parameters and schemas before invocation.
    /// Useful for debugging tool call failures or understanding tool capabilities.
    ///
    /// USE CASE:
    /// - Agent queries tool spec to understand parameters: tool:explain(name="read_file")
    /// - User debugs tool call failure by examining expected schema
    /// - UI displays available tools with descriptions and parameters
    ///
    /// BACKWARD COMPATIBILITY: Strips "user__" prefix from tool names.
    /// Old tool naming convention: "user__read_file" → "read_file"
    /// WHY: Legacy MCP plugin tool names had user__ prefix.
    ///
    /// SECURITY NOTE: No actor restrictions (any actor may query tool specs).
    /// Read-only operation, no side effects.
    ///
    /// ARGUMENTS:
    /// - `name` (string, required): Tool name (e.g., "read_file")
    /// - `room` (string, optional): Room identifier (default: "main")
    /// - `source` (string, optional): Tool source (default: "external")
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{room, source, name, summary, description, schema_json}` on success
    /// - `E_INVALID_ARGS` if name is empty or source is unsupported
    /// - `E_NOT_FOUND` if tool does not exist in registry
    /// - `E_IO` if database query fails
    /// - `E_CANCELLED` if context is cancelled mid-execution
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // ---------------------------------------------------------------------
        // PHASE 1: Validation & Argument Parsing
        // ---------------------------------------------------------------------
        // WHY: Validate kernel state and parse arguments before database query.
        ctx.check_cancelled()?;

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let Some(store) = k.store() else {
            return Err(KernelError::internal("kernel store not attached"));
        };

        let args: ToolExplainArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let tool_name = args.name.trim();
        if tool_name.is_empty() {
            return Err(KernelError::invalid_args("name is required"));
        }

        // WHY: Historically, some clients used a "user__" prefix in tool names.
        // We prefer an exact lookup, but fall back to stripping the prefix for
        // backward compatibility.
        let tool_name_raw = tool_name;
        let tool_name_stripped = tool_name_raw
            .strip_prefix("user__")
            .unwrap_or(tool_name_raw);

        // WHY: Default room to "main" if unspecified. Most tools are registered
        // in the main room.
        let room = args
            .room
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("main");

        // WHY: Default source to "external" (user-registered tools). Future
        // sources: "internal" (built-in syscalls), "plugin" (MCP plugins).
        let source = args
            .source
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("external");

        // WHY: Only "external" source is currently supported. Internal/plugin
        // sources are reserved for future use.
        if source != "external" {
            return Err(KernelError::invalid_args("unsupported source"));
        }

        // ---------------------------------------------------------------------
        // PHASE 2: Database Lookup
        // ---------------------------------------------------------------------
        // WHY: Query SQLite tool_registry table for tool specification.
        // Returns full spec including JSON schema.
        // Try exact name first; if not found and the name had a user__ prefix,
        // retry using the stripped name.
        let mut found = match store.get_tool(room, source, tool_name_raw).await {
            Ok(v) => v,
            Err(e) => return Err(KernelError::io(format!("db error: {e}"))),
        };
        if found.is_none() && tool_name_raw != tool_name_stripped {
            found = match store.get_tool(room, source, tool_name_stripped).await {
                Ok(v) => v,
                Err(e) => return Err(KernelError::io(format!("db error: {e}"))),
            };
        }

        match found {
            Some(t) => {
                // WHY: Return full tool specification including JSON schema.
                // Enables callers to inspect parameters and validation rules.
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({
                            "room": room,
                            "source": source,
                            "name": t.name,
                            "summary": t.summary,
                            "description": t.description,
                            "schema_json": t.schema_json,
                        }),
                    ))
                    .await;
                Ok(())
            }
            None => {
                // WHY: Return E_NOT_FOUND if tool does not exist in registry.
                // Helps callers distinguish between missing tool vs database error.
                Err(KernelError::not_found(format!(
                    "tool not found: {} (source={})",
                    tool_name_raw, source
                )))
            }
        }
    }
}
