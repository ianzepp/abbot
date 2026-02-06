//! Memory - Semantic Search and Recall
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides semantic search over conversation transcripts using
//! vector embeddings. It enables agents to recall past discussions, decisions,
//! and context that aren't stored in structured LTM/STM.
//!
//! **Memory hierarchy in Abbot:**
//! - **STM (Short-Term Memory)**: Current session state (cleared on restart)
//! - **LTM (Long-Term Memory)**: Structured facts and decisions (persistent)
//! - **Recall (Vector Memory)**: Semantic search over all past conversations
//!
//! **Integration points:**
//! - `Search` service (from `recall` module) performs vector similarity search
//! - `Indexer` ingests conversation transcripts into sqlite-vec database
//! - `Ollama` provides local embedding model (nomic-embed-text)
//! - Agents query with natural language: "why did we decide X?"
//!
//! **Storage:**
//! - Vector database: sqlite-vec extension for efficient similarity search
//! - Embeddings: 768-dimensional vectors from nomic-embed-text model
//! - Chunked transcripts: Split into semantic segments for indexing
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Natural language queries**: Agents search with questions, not keywords
//! - **Semantic similarity**: Vector search finds relevant context even without exact match
//! - **Optional dependency**: Recall is disabled if embedding model unavailable
//! - **Read-only access**: All agents can search memories, no mutation needed
//! - **Local-first**: Uses local Ollama model, no external API calls
//!
//! WHY RECALL EXISTS
//! =================
//! LTM stores structured facts, but can't capture entire conversation history.
//! Recall solves this by enabling fuzzy search over past discussions:
//!
//! - "What did we discuss about error handling?"
//! - "Why did we choose architecture pattern X?"
//! - "What was the user's concern about performance?"
//!
//! Without recall, agents must rely solely on LTM (limited size) or current
//! session context (cleared on restart). Recall bridges the gap by making
//! entire conversation history searchable.
//!
//! TRADE-OFFS
//! ==========
//! 1. **Vector Search vs. Full-Text Search**
//!    - CHOSEN: Vector embeddings with semantic similarity
//!    - REJECTED: Traditional keyword search (BM25, FTS5)
//!    - WHY: Semantic search understands meaning, not just word overlap
//!    - IMPLICATION: Requires embedding model (Ollama) and larger index
//!
//! 2. **Local Model vs. Cloud API**
//!    - CHOSEN: Local Ollama with nomic-embed-text
//!    - WHY: Privacy (no external API calls), low latency, no API costs
//!    - IMPLICATION: Requires Ollama installation, limited to local compute
//!
//! 3. **Optional vs. Required**
//!    - CHOSEN: Recall is optional (gracefully degrades if unavailable)
//!    - WHY: Abbot should work without Ollama for basic tasks
//!    - IMPLICATION: Agents get `E_DISABLED` error if recall not configured
//!
//! CONCURRENCY
//! ===========
//! - Vector searches are read-only and safe for concurrent access
//! - sqlite-vec handles multi-reader locking internally
//! - No write operations in this namespace (indexing is separate service)
//!
//! PERFORMANCE
//! ===========
//! - Vector search is O(n) scan over embeddings (not indexed by default)
//! - TRADE-OFF: Acceptable because typical transcript DB is < 10k chunks
//! - Results are limited (default 5, max 20) to bound computation
//! - Content is truncated (1200 chars) to bound response size
//!
//! SECURITY MODEL
//! ==============
//! - Read-only access: All agents can search (no mutation permission needed)
//! - WHY: Conversation history is already accessible to agents in their context
//! - Private to workspace: Each workspace has separate transcript index
//!
//! SYSCALLS
//! ========
//! - `memory:recall` - Search conversation history with natural language query

mod recall;

pub use recall::MemoryRecall;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

/// Register memory syscalls without semantic search (recall disabled).
///
/// WHY: Allows kernel to boot without Ollama dependency. Agents will receive
/// `E_DISABLED` error when attempting recall operations.
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(MemoryRecall::new()));
}

/// Register memory syscalls with semantic search enabled.
///
/// WHY: Provides fully-functional recall when Ollama embedding model is available.
/// Separates registration from syscall construction for dependency injection.
pub fn register_with_search(
    dispatcher: &mut KernelDispatcher,
    search: Arc<crate::recall::Search>,
) {
    dispatcher.register(Arc::new(MemoryRecall::with_search(search)));
}
