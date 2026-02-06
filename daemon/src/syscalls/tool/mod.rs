//! Tool - External tool registration and execution coordination
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides syscalls for managing external tools (those executed outside the
//! kernel by external executors like UI, daemon, or plugins). It implements a **registration
//! and delivery protocol** that decouples tool specification from tool execution.
//!
//! **Critical design decisions:**
//! - Tool specifications stored in SQLite (`tool_registry` table) via Store
//! - Tool catalog cached in-memory by `ExternalToolManager` for fast lookup
//! - Tool call delivery via oneshot channel (register_pending → deliver_result)
//! - Scope-based isolation (tools registered per scope: "main", custom scopes)
//! - Idempotent result delivery (recent_completed cache prevents duplicate delivery errors)
//!
//! **Integration points:**
//! - `Store.replace_external_tools()` - Persists tool specifications to SQLite
//! - `ExternalToolManager` - In-memory cache + pending call coordination
//! - `dispatch_tool()` in syscalls/dispatch.rs - Maps tool names to syscalls
//! - Hand/Head agents - Register tools, then invoke via LLM tool calls
//!
//! **Tool lifecycle:**
//! 1. **Registration** (tool:register):
//!    - External executor registers tools with JSON schema specifications
//!    - Tools persisted to SQLite + cached in ExternalToolManager
//!    - Tools become available to LLMs via OpenAI function calling format
//!
//! 2. **Discovery** (tool:explain):
//!    - Agents query tool specifications by name
//!    - Returns JSON schema, description, parameters
//!    - Used for debugging tool call failures
//!
//! 3. **Execution** (hand agent via dispatch_tool):
//!    - LLM generates tool call with name + arguments
//!    - dispatch_tool() maps tool name to syscall name (e.g., "read_file" → "fs:read")
//!    - Syscall executes and returns result to agent
//!
//! 4. **Result delivery** (tool:result/tool:deliver_result):
//!    - For external tools (not mapped to syscalls), result delivered out-of-band
//!    - External executor calls tool:result with tool_call_id + output
//!    - ExternalToolManager routes result via oneshot channel to waiting agent
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Specification vs Execution**: Tool specs define the contract, executors provide the implementation
//! - **Scope isolation**: Each scope (main, custom) has independent tool catalog
//! - **Persistence + Cache**: Tools survive daemon restarts (SQLite) but are fast to lookup (in-memory)
//! - **Idempotent delivery**: Result delivery can be retried without error (recent_completed cache)
//! - **No actor restrictions**: Any actor may register or query tools (authorization happens in syscall dispatch)
//!
//! TOOL CATALOG MANAGEMENT
//! =======================
//! **Registration (tool:register):**
//! - Replaces entire tool catalog for a scope (not incremental)
//! - WHY: Simplifies synchronization (external executor owns catalog, no merge logic needed)
//! - Schema: `[{name, summary, description, schema_json}, ...]`
//! - Stored in `tool_registry` table with columns: `scope, source, name, summary, description, schema_json`
//!
//! **Discovery (tool:explain):**
//! - Lookup single tool by name + scope + source
//! - WHY: Enables agents to inspect tool specifications for debugging
//! - Returns full tool specification including JSON schema
//!
//! **Source types:**
//! - "external": Tools registered via tool:register (persisted in SQLite)
//! - Future: "internal" (built-in syscalls), "plugin" (MCP plugins)
//!
//! TOOL DISPATCH INTEGRATION
//! =========================
//! **How tools are invoked:**
//! 1. LLM generates tool call: `{function: {name: "read_file", arguments: '{"path": "auth.rs"}'}}`
//! 2. Hand agent calls `dispatch_tool()` with tool name + arguments
//! 3. `tool_to_syscall()` maps tool name to syscall name (see syscalls/dispatch.rs)
//! 4. Kernel dispatcher routes to appropriate syscall (e.g., fs:read)
//! 5. Result returned as JSON: `{"ok": true, "data": {...}}` or `{"ok": false, "error": {...}}`
//!
//! **Tool name → Syscall name mapping:**
//! - Defined in `tool_spec!()` macro in syscalls/dispatch.rs
//! - Examples: `read_file → fs:read`, `write_file → fs:write`, `git_status → git:status`
//! - WHY separate tool names: LLM-friendly names vs kernel-internal syscall names
//!
//! **External tool execution:**
//! - For tools NOT mapped to syscalls (custom external tools)
//! - External executor receives tool call, executes, delivers result via tool:result
//! - WHY: Enables user-defined tools without modifying kernel code
//!
//! REGISTERED SYSCALLS
//! ===================
//! - `tool:register` - Register/replace tool catalog for a scope
//! - `tool:explain` - Lookup tool specification by name
//! - `tool:result` - Deliver execution result for external tool (preferred)
//! - `tool:deliver_result` - Backward-compatible alias for tool:result
//!
//! SECURITY MODEL
//! ==============
//! **No explicit authorization at registration:**
//! - Any actor may register tools (no actor restriction)
//! - WHY: Authorization happens at syscall dispatch time (e.g., fs:read checks VFS permissions)
//! - External executors are trusted (they control tool catalog)
//!
//! **Tool execution security:**
//! - Tool dispatch goes through normal syscall authorization (actor, mutation checks)
//! - External tools are executed by external executor (outside kernel sandbox)
//! - Tool result delivery validated by ExternalToolManager (tool_call_id must be pending)
//!
//! CONCURRENCY
//! ===========
//! - Tool catalog updates via RwLock (concurrent reads, exclusive writes)
//! - Pending tool calls via Mutex (oneshot channel registration/delivery is atomic)
//! - Result delivery is idempotent (recent_completed cache prevents duplicate delivery errors)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Replace vs Incremental Updates**
//!    - CHOSEN: tool:register replaces entire catalog
//!    - WHY: Simpler synchronization, external executor owns catalog
//!    - IMPLICATION: Cannot partially update catalog (must send all tools each time)
//!
//! 2. **Persistence Layer**
//!    - CHOSEN: SQLite storage + in-memory cache
//!    - WHY: Tools survive daemon restarts, but lookup is fast
//!    - IMPLICATION: Must keep SQLite and cache synchronized
//!
//! 3. **Result Delivery Idempotency**
//!    - CHOSEN: Cache recent completions (256 entries)
//!    - WHY: External executors may retry result delivery after reconnect
//!    - IMPLICATION: Memory overhead for tracking recent completions

mod deliver_result;
mod explain;
mod register;
mod result;

pub use deliver_result::ToolDeliverResult;
pub use explain::ToolExplain;
pub use register::ToolRegister;
pub use result::ToolResult;

use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, SyscallContext};
use crate::runtime::Kernel;

// =============================================================================
// SHARED RESULT DELIVERY
// =============================================================================
//
// WHY: Both tool:result and tool:deliver_result use the same implementation.
// tool:deliver_result is a backward-compatible alias for tool:result.
//
// This function delivers tool execution results to waiting agents via the
// ExternalToolManager's oneshot channel mechanism. The flow is:
// 1. Agent registers pending tool call (register_pending creates oneshot channel)
// 2. External executor runs tool
// 3. External executor calls this function with tool_call_id + output
// 4. ExternalToolManager delivers result via oneshot channel to waiting agent

/// Deliver tool execution result to waiting agent.
///
/// WHY: Shared implementation for tool:result and tool:deliver_result syscalls.
/// Routes execution results from external executor to waiting agent.
///
/// IDEMPOTENCY: Result delivery is idempotent via recent_completed cache.
/// If tool_call_id was recently completed, duplicate delivery succeeds silently.
/// This prevents errors when external executor retries after reconnect.
///
/// SECURITY NOTE: No actor restrictions (any actor may deliver results).
/// Tool call must be registered as pending (prevents spurious result injection).
pub(crate) async fn deliver_result(
    ctx: &SyscallContext,
    data: serde_json::Value,
    tx: mpsc::Sender<Frame>,
) -> Result<(), KernelError> {
    // -------------------------------------------------------------------------
    // PHASE 1: Validation
    // -------------------------------------------------------------------------
    // WHY: Validate scope and tool_call_id before attempting delivery.
    // Early validation provides clear error messages for malformed requests.
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

    let tool_call_id = data
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if tool_call_id.is_empty() {
        return Err(KernelError::invalid_args("tool_call_id is required"));
    }

    let output = data
        .get("output")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // -------------------------------------------------------------------------
    // PHASE 2: Result Delivery
    // -------------------------------------------------------------------------
    // WHY: Deliver result via ExternalToolManager's oneshot channel.
    // This routes the result to the agent that registered the pending tool call.
    //
    // IDEMPOTENCY: If tool_call_id is not pending but was recently completed,
    // delivery succeeds silently (prevents retry errors after reconnect).
    k.external_tools()
        .deliver_result(scope, tool_call_id, output)
        .await
        .map_err(KernelError::invalid_args)?;

    // -------------------------------------------------------------------------
    // PHASE 3: Acknowledgment
    // -------------------------------------------------------------------------
    // WHY: Return Frame::ok confirming delivery. Bump activity to prevent
    // idle timeout while external tools are executing.
    k.bump_activity();
    let _ = tx
        .send(Frame::ok(ctx.call_id, json!({"delivered": true})))
        .await;
    Ok(())
}

// =============================================================================
// SYSCALL REGISTRATION
// =============================================================================
//
// WHY: Register all tool namespace syscalls with the kernel dispatcher.
// Called during kernel initialization (see syscalls/mod.rs).

/// Register all tool namespace syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration for the tool namespace.
/// Called once during kernel initialization.
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(ToolResult::new()));
    dispatcher.register(Arc::new(ToolDeliverResult::new()));
    dispatcher.register(Arc::new(ToolRegister::new()));
    dispatcher.register(Arc::new(ToolExplain::new()));
}
