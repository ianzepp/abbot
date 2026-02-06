//! Memory:Recall - Semantic search over conversation transcripts
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall performs semantic vector search over indexed conversation transcripts,
//! enabling agents to recall past discussions with natural language queries.
//!
//! **Search pipeline:**
//! 1. Query text → Embedding model (nomic-embed-text via Ollama)
//! 2. Query vector → Similarity search (cosine distance in sqlite-vec)
//! 3. Top-K results → Truncated content + metadata
//!
//! **Integration points:**
//! - `Search` service provides vector similarity search
//! - sqlite-vec extension stores and queries embeddings
//! - Ollama provides local embedding model
//! - Results include file path, source, and relevance score (distance)
//!
//! **Result structure:**
//! - `distance`: Cosine similarity (lower = more relevant)
//! - `source`: Origin of content (transcript file, LTM, etc.)
//! - `file_path`: Absolute path to source file
//! - `project_path`: Relative path within workspace
//! - `content`: Truncated to 1200 characters
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Natural language queries**: Agents ask questions, not keywords
//! - **Relevance-ranked results**: Cosine similarity provides confidence score
//! - **Graceful degradation**: Returns `E_DISABLED` if search unavailable
//! - **Bounded results**: Limits prevent overwhelming agent context window
//! - **Content truncation**: Prevents huge result payloads
//!
//! SECURITY MODEL
//! ==============
//! - No mutation permission required (read-only access)
//! - WHY: Conversation history is already accessible to agents in their context
//! - Workspace-scoped: Each workspace has separate transcript index
//! - WHY: Prevents agents from accessing other workspaces' conversations
//!
//! PERFORMANCE
//! ===========
//! - Vector search is O(n) scan over embeddings (not indexed)
//! - TRADE-OFF: Acceptable for typical transcript DB size (< 10k chunks)
//! - Results limited: default 5, max 20 (bounded computation)
//! - Content truncated: 1200 chars (bounded response size)
//! - Query embedding: ~50ms via local Ollama
//!
//! TRADE-OFFS
//! ==========
//! 1. **Content Truncation**
//!    - CHOSEN: 1200 character limit per result
//!    - WHY: Prevents huge payloads overwhelming agent context
//!    - IMPLICATION: Agents may not see full conversation snippet
//!
//! 2. **Result Limit**
//!    - CHOSEN: Default 5 results, max 20
//!    - WHY: Balances relevance diversity with context window size
//!    - IMPLICATION: Highly relevant results may be excluded if beyond limit
//!
//! 3. **No Filtering by Date/Source**
//!    - CHOSEN: Simple relevance ranking only
//!    - WHY: Reduces API complexity, lets Search service handle logic
//!    - IMPLICATION: Old conversations rank equally with recent ones
//!
//! CONCURRENCY
//! ===========
//! - Read-only operation, safe for concurrent access
//! - Search service handles sqlite locking internally
//! - No shared mutable state in syscall

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::recall::Search;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `memory:recall` syscall.
///
/// WHY: Simple query interface with optional limit for flexibility.
#[derive(Debug, Deserialize)]
struct MemoryRecallArgs {
    /// Natural language search query.
    ///
    /// WHY: Agents phrase queries as questions ("why did we decide X?") rather
    /// than keywords. Vector embeddings capture semantic meaning.
    query: String,

    /// Maximum number of results to return.
    ///
    /// WHY: Optional to allow callers to override default (5) for broader
    /// or narrower result sets. Clamped to [1, 20] to prevent abuse.
    #[serde(default)]
    limit: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for semantic search over conversation transcripts.
///
/// WHY: Encapsulates optional Search dependency, allowing kernel to boot
/// without Ollama and gracefully degrade recall functionality.
pub struct MemoryRecall {
    /// Optional semantic search service.
    ///
    /// WHY: None if Ollama unavailable, Some if search is configured.
    /// Enables dependency injection for testing and optional features.
    search: Option<Arc<Search>>,
}

impl MemoryRecall {
    /// Create a new `MemoryRecall` syscall without search (recall disabled).
    ///
    /// WHY: Allows kernel to boot without Ollama. Agents receive `E_DISABLED`
    /// when attempting recall operations.
    pub fn new() -> Self {
        Self { search: None }
    }

    /// Create a `MemoryRecall` syscall with semantic search enabled.
    ///
    /// WHY: Dependency injection pattern for Search service. Enables testing
    /// with mock search implementations.
    pub fn with_search(search: Arc<Search>) -> Self {
        Self {
            search: Some(search),
        }
    }
}

#[async_trait]
impl Syscall for MemoryRecall {
    fn name(&self) -> &'static str {
        "memory:recall"
    }

    /// Search conversation transcripts with natural language query.
    ///
    /// WHY: Enables agents to recall past discussions using semantic search,
    /// bridging the gap between structured LTM and full conversation history.
    ///
    /// USE CASE: Invoked by agents to answer questions about past context:
    /// - "What did we discuss about error handling?"
    /// - "Why did we choose architecture pattern X?"
    /// - "What was the user's concern about performance?"
    ///
    /// SECURITY NOTE: No mutation permission required (read-only access).
    /// Workspace-scoped: agents can only search their own workspace's transcripts.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{results: [{distance, source, file_path, project_path, content}]}`
    /// - `E_DISABLED` if search service not configured (Ollama unavailable)
    /// - `E_INVALID_ARGS` if query is empty or malformed
    /// - `E_IO` if search operation fails (embedding or database error)
    /// - `E_CANCELLED` if parent context cancels during execution
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // ---------------------------------------------------------------------
        // PHASE 1: Cancellation Check
        // ---------------------------------------------------------------------
        // WHY: Early exit prevents wasted work on already-cancelled tasks.
        ctx.check_cancelled()?;

        // ---------------------------------------------------------------------
        // PHASE 2: Argument Parsing & Validation
        // ---------------------------------------------------------------------
        // WHY: Validate arguments before search operation to provide clear
        // error messages for malformed requests.
        let args: MemoryRecallArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let query = args.query.trim();
        if query.is_empty() {
            return Err(KernelError::invalid_args("query is required"));
        }

        // ---------------------------------------------------------------------
        // PHASE 3: Search Service Availability Check
        // ---------------------------------------------------------------------
        // WHY: Graceful degradation if Ollama unavailable. Provides clear
        // error message instead of panic or cryptic failure.
        let Some(search) = &self.search else {
            return Err(KernelError::disabled("memory search is not available"));
        };

        // ---------------------------------------------------------------------
        // PHASE 4: Result Limit Calculation
        // ---------------------------------------------------------------------
        // WHY: Clamp to [1, 20] to prevent abuse while allowing flexibility.
        // - Default 5: Balances relevance diversity with context size
        // - Max 20: Prevents overwhelming agent context window
        // - Min 1: Ensures at least one result if available
        let limit = args.limit.unwrap_or(5).clamp(1, 20);

        // ---------------------------------------------------------------------
        // PHASE 5: Vector Search Execution
        // ---------------------------------------------------------------------
        // WHY: Delegate to Search service for embedding and similarity search.
        // Maps I/O errors to KernelError for consistent error handling.
        //
        // PERFORMANCE: Vector search is O(n) over embeddings, typically 50-200ms
        // for ~10k chunks with local Ollama embedding (~50ms) + sqlite scan.
        let results = search
            .query(query, limit)
            .await
            .map_err(|e| KernelError::io(format!("recall error: {e}")))?;

        // ---------------------------------------------------------------------
        // PHASE 6: Result Formatting & Content Truncation
        // ---------------------------------------------------------------------
        // WHY: Truncate content to 1200 characters to prevent huge payloads.
        // Include metadata (distance, source, paths) for debugging and relevance scoring.
        let out: Vec<serde_json::Value> = results
            .into_iter()
            .map(|r| {
                // WHY: Character-based truncation (not byte-based) to prevent
                // splitting multi-byte UTF-8 sequences. 1200 chars ≈ 2-3 paragraphs.
                let content: String = r.content.chars().take(1200).collect();
                json!({
                    "distance": r.distance,      // WHY: Cosine similarity (lower = more relevant)
                    "source": r.source,           // WHY: Origin (transcript, LTM, etc.)
                    "file_path": r.file_path,     // WHY: Absolute path for debugging
                    "project_path": r.project_path, // WHY: Relative path for display
                    "content": content            // WHY: Truncated transcript text
                })
            })
            .collect();

        // ---------------------------------------------------------------------
        // PHASE 7: Response
        // ---------------------------------------------------------------------
        // WHY: Return results array for agent consumption. Empty array if no matches.
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"results": out})))
            .await;

        Ok(())
    }
}
