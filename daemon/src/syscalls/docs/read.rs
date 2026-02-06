//! Docs:Read - Retrieve full content of a specific document
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall retrieves the complete markdown content of a specific embedded
//! document by name. It provides read-only access to system documentation for
//! agents, enabling them to reference architecture details, syscall specifications,
//! and tool usage guidelines.
//!
//! **Integration points:**
//! - Reads from compile-time `DOCS` constant defined in parent module
//! - Returns full document content as markdown string
//! - Typically invoked after `docs:list` to retrieve specific documentation
//!
//! **Frame protocol:**
//! - Emits single `Frame::ok` with document name, content, and size
//! - Returns `E_NOT_FOUND` if document name doesn't match catalog
//! - No streaming (documents are small enough for single frame)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Name-based lookup**: Documents are identified by stable string names
//! - **Full content delivery**: Returns complete markdown, not excerpts
//! - **Universal access**: No actor restrictions - all agents can read documentation
//! - **Helpful errors**: Suggests available documents when lookup fails
//!
//! SECURITY MODEL
//! ==============
//! - **No actor restrictions**: All agents (head, hand, room) may read documentation
//! - **Read-only operation**: Cannot modify documentation content
//! - **Static catalog**: Only pre-approved documents are accessible (no path traversal)
//! - **Bounded output**: Individual documents are <200KB (safe for single frame)
//!
//! PERFORMANCE
//! ===========
//! - Lookup is O(n) linear search through document array (n typically <10)
//! - Content is already in memory (embedded at compile time)
//! - No I/O operations
//! - Single allocation for JSON response

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::DOCS;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for reading a specific documentation file.
///
/// WHY: Provides agents with full access to system documentation content,
/// enabling them to reference architecture details, syscall specifications,
/// and best practices during operation.
pub struct DocsRead;

impl DocsRead {
    /// Create a new `DocsRead` syscall.
    ///
    /// WHY: Zero-config constructor - all state is in the shared `DOCS` constant.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for DocsRead {
    fn name(&self) -> &'static str {
        "docs:read"
    }

    /// Read the full content of a specific documentation file.
    ///
    /// WHY: Enables agents to access detailed system documentation for reference
    /// during operation. Agents typically call `docs:list` first to discover
    /// available documents, then `docs:read` to retrieve specific content.
    ///
    /// USE CASE: Invoked by all agent types when they need to reference system
    /// architecture, syscall specifications, or tool usage guidelines. Common
    /// scenarios include:
    /// - Head agents reading "syscalls" doc before complex operations
    /// - Hand agents reading "tools" doc to understand LLM tool specifications
    /// - Room agents reading "architecture" doc to understand system design
    ///
    /// ARGUMENTS:
    /// - `name` (string, required) - Document identifier (e.g., "architecture", "syscalls")
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{name, content, size}` if document exists
    /// - `E_INVALID_ARGS` if name is missing or empty
    /// - `E_NOT_FOUND` if document doesn't exist (includes available document list)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Cancellation Check
        // =====================================================================
        // WHY: Respect context cancellation before performing work. Even though
        // lookup is fast, this maintains consistent cancellation semantics.
        ctx.check_cancelled()?;

        // =====================================================================
        // PHASE 2: Argument Extraction & Validation
        // =====================================================================
        // WHY: Extract and validate the document name before lookup. Trimming
        // whitespace prevents spurious lookup failures from malformed input.
        //
        // VALIDATION: Name must be non-empty after trimming. Empty name would
        // never match catalog, so fail fast with clear error message.
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if name.is_empty() {
            return Err(KernelError::invalid_args("name is required"));
        }

        // =====================================================================
        // PHASE 3: Document Lookup
        // =====================================================================
        // WHY: Search embedded document catalog for matching name. Linear search
        // is acceptable since catalog is small (<10 documents).
        //
        // PERFORMANCE: O(n) where n is number of documents. Could use HashMap
        // for O(1) lookup, but current size doesn't justify the overhead.
        let doc = DOCS.iter().find(|d| d.name == name);

        // =====================================================================
        // PHASE 4: Response or Error
        // =====================================================================
        // WHY: Return document content if found, or helpful error with suggestions
        // if not found. Including available document list helps agents correct
        // typos or discover alternative documentation.
        match doc {
            Some(d) => {
                // WHY: Include name, content, and size in response. Size helps
                // clients estimate processing time or truncate display.
                let _ = tx
                    .send(Frame::ok(
                        ctx.call_id,
                        json!({
                            "name": d.name,
                            "content": d.content,
                            "size": d.content.len(),
                        }),
                    ))
                    .await;
                Ok(())
            }
            None => {
                // WHY: Include list of available documents in error message to
                // help agents discover correct name or nearby alternatives.
                let available: Vec<&str> = DOCS.iter().map(|d| d.name).collect();
                Err(KernelError::not_found(format!(
                    "document '{}' not found (available: {})",
                    name,
                    available.join(", ")
                )))
            }
        }
    }
}
