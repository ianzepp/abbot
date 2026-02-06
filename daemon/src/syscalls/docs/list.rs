//! Docs:List - Enumerate available documentation files
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides discovery of available system documentation. It returns
//! metadata about each embedded document (name and size), enabling agents to
//! understand what documentation exists before reading specific files.
//!
//! **Integration points:**
//! - Reads from compile-time `DOCS` constant defined in parent module
//! - Returns structured JSON array of document metadata
//! - Typically invoked by agents during initialization or when exploring capabilities
//!
//! **Frame protocol:**
//! - Emits single `Frame::ok` with array of document entries
//! - No streaming (entire catalog fits in single frame)
//! - Never fails (DOCS is guaranteed valid at compile time)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **No arguments required**: Listing is unconditional and always complete
//! - **Metadata over content**: Returns document names and sizes, not full content
//! - **Universal access**: No actor restrictions - all agents can list documentation
//! - **Deterministic output**: Always returns same result (static catalog)
//!
//! SECURITY MODEL
//! ==============
//! - **No actor restrictions**: All agents (head, hand, room) may list documentation
//! - **Read-only operation**: Cannot modify documentation catalog
//! - **No filesystem access**: Reads from embedded binary data only
//! - **Bounded output**: Small, fixed-size response (currently 3 documents)
//!
//! PERFORMANCE
//! ===========
//! - O(n) in number of documents, where n is typically <10
//! - Single heap allocation for result vector
//! - No I/O operations
//! - Size calculation is strlen (O(1) for embedded strings)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::DOCS;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for listing available documentation files.
///
/// WHY: Enables agents to discover what documentation exists before reading
/// specific documents. Provides metadata (name, size) without loading full content.
pub struct DocsList;

impl DocsList {
    /// Create a new `DocsList` syscall.
    ///
    /// WHY: Zero-config constructor - all state is in the shared `DOCS` constant.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for DocsList {
    fn name(&self) -> &'static str {
        "docs:list"
    }

    /// List all available documentation files with metadata.
    ///
    /// WHY: Provides agents with a catalog of available documentation, enabling
    /// discovery of system capabilities and reference materials. Agents typically
    /// call this before `docs:read` to understand what documentation exists.
    ///
    /// USE CASE: Invoked by all agent types (head, hand, room) during initialization
    /// or when exploring system capabilities. LLMs use this to discover relevant
    /// documentation before performing complex operations.
    ///
    /// ARGUMENTS: None required - listing is unconditional.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{docs: [{name, size}, ...]}` array
    /// - Never fails (documentation catalog is guaranteed valid at compile time)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Cancellation Check
        // =====================================================================
        // WHY: Respect context cancellation before performing work, even though
        // this operation is fast. Prevents wasted work on cancelled tasks.
        ctx.check_cancelled()?;

        // =====================================================================
        // PHASE 2: Build Document Metadata
        // =====================================================================
        // WHY: Transform embedded document entries into JSON metadata objects.
        // Includes name (for `docs:read` lookup) and size (for client estimation).
        //
        // PERFORMANCE: Single allocation for result vector. Size calculation is
        // O(1) since embedded strings have precomputed lengths.
        let docs: Vec<serde_json::Value> = DOCS
            .iter()
            .map(|d| {
                json!({
                    "name": d.name,
                    "size": d.content.len(),
                })
            })
            .collect();

        // =====================================================================
        // PHASE 3: Send Response
        // =====================================================================
        // WHY: Emit single Frame::ok with complete catalog. No streaming needed
        // since catalog is small (currently 3 documents, <1KB metadata).
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({ "docs": docs })))
            .await;

        Ok(())
    }
}
