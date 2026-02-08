//! Docs Namespace - Documentation retrieval and search operations
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides read-only access to embedded system documentation for agents.
//! All documentation content is compiled into the binary as string literals using `include_str!`,
//! eliminating runtime filesystem dependencies and ensuring documentation is always available.
//!
//! **Integration points:**
//! - Documentation markdown files stored in `src/docs/` directory
//! - Embedded at compile time via `include_str!` macros
//! - Available to all agent types (head, hand, room) without restrictions
//! - Integrated with LLM tool definitions for agent discovery
//!
//! **Key operations:**
//! - `docs:list` - Enumerate available documentation files
//! - `docs:read` - Retrieve full content of a specific document
//! - `docs:search` - Full-text search across all documentation with line-level results
//!
//! **Document catalog:**
//! - `architecture` - System architecture overview, agent types, concurrency model
//! - `syscalls` - Complete syscall reference with examples and security notes
//! - `tools` - LLM tool specifications and usage guidelines
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Compile-time embedding**: Documentation is part of the binary, never stale or missing
//! - **Universal access**: All agents can read documentation without security restrictions
//! - **Searchable content**: Full-text search enables agents to discover relevant information
//! - **Structured catalog**: Fixed set of curated documents prevents information overload
//! - **Immutable content**: Documentation cannot be modified at runtime (security guarantee)
//!
//! SECURITY MODEL
//! ==============
//! - **Read-only access**: No syscalls in this namespace modify state
//! - **No actor restrictions**: All agents (head, hand, room) can access documentation
//! - **No filesystem access**: Content is embedded, preventing path traversal attacks
//! - **Bounded search**: Search results are limited to prevent resource exhaustion
//! - **Static catalog**: Only pre-approved documents are available (no dynamic loading)
//!
//! PERFORMANCE
//! ===========
//! - Documentation strings are stored in read-only binary sections (zero heap allocation)
//! - Search is in-memory linear scan (acceptable for <10 documents, <500KB total)
//! - No I/O operations during syscall execution
//!
//! TRADE-OFFS
//! ==========
//! 1. **Embedded vs. Filesystem Storage**
//!    - CHOSEN: Compile-time embedding via `include_str!`
//!    - REJECTED: Runtime loading from disk
//!    - WHY: Eliminates runtime dependencies, prevents documentation from being out-of-sync
//!    - IMPLICATION: Documentation updates require binary recompilation
//!
//! 2. **Static vs. Dynamic Catalog**
//!    - CHOSEN: Fixed set of documents defined in `DOCS` constant
//!    - REJECTED: Directory scanning or plugin-based documentation
//!    - WHY: Curated documentation prevents information overload for LLMs
//!    - IMPLICATION: Adding new documents requires code changes
//!
//! 3. **Search Algorithm**
//!    - CHOSEN: Simple case-insensitive substring matching
//!    - REJECTED: Full-text indexing (inverted index, TF-IDF, etc.)
//!    - WHY: Documentation corpus is small (<500KB), indexing overhead not justified
//!    - IMPLICATION: Search is O(n) in total content size, but fast enough in practice

mod list;
mod read;
mod search;

pub use list::DocsList;
pub use read::DocsRead;
pub use search::DocsSearch;

// =============================================================================
// DOCUMENTATION CATALOG
// =============================================================================
//
// The documentation catalog is a static array of embedded markdown files.
// Each document is compiled into the binary at build time using `include_str!`.
//
// WHY: Compile-time embedding ensures documentation is always available and
// matches the binary version (no version skew). The `Doc` struct provides
// metadata (name) alongside content for enumeration and search operations.
//
// SECURITY: Only files explicitly listed here are accessible. There is no
// directory scanning or dynamic loading, preventing path traversal attacks.

/// Documentation entry with embedded content.
///
/// WHY: Pairs document name (for lookup) with embedded content string.
/// The name is used as the primary key for `docs:read` operations.
pub(crate) struct Doc {
    /// Document identifier (e.g., "architecture", "syscalls").
    ///
    /// WHY: Stable identifier for lookups, independent of filesystem path.
    pub name: &'static str,

    /// Embedded markdown content.
    ///
    /// WHY: 'static lifetime means content lives in read-only binary section,
    /// requiring zero heap allocation and surviving for program lifetime.
    pub content: &'static str,
}

/// Static catalog of all available documentation.
///
/// WHY: Compile-time constant ensures documentation set is known at build time.
/// Agents can reliably enumerate available docs via `docs:list`.
///
/// TRADE-OFF: Adding new documents requires code changes and recompilation,
/// but this ensures documentation is curated and prevents information overload.
pub(crate) const DOCS: &[Doc] = &[
    Doc {
        name: "architecture",
        content: include_str!("../../prompts/docs/architecture.md"),
    },
    Doc {
        name: "syscalls",
        content: include_str!("../../prompts/docs/syscalls.md"),
    },
    Doc {
        name: "tools",
        content: include_str!("../../prompts/docs/tools.md"),
    },
];

// =============================================================================
// REGISTRATION
// =============================================================================

/// Register all documentation syscalls with the kernel dispatcher.
///
/// WHY: Single registration point for all docs namespace syscalls. Called
/// during kernel initialization to populate the syscall routing table.
///
/// SYSCALLS REGISTERED:
/// - `docs:list` - Enumerate available documentation files
/// - `docs:search` - Full-text search across all documentation
/// - `docs:read` - Retrieve specific document by name
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(DocsList::new()));
    dispatcher.register(Arc::new(DocsSearch::new()));
    dispatcher.register(Arc::new(DocsRead::new()));
}
