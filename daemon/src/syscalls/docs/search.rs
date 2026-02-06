//! Docs:Search - Full-text search across all documentation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall performs case-insensitive substring search across all embedded
//! documentation content, returning matching lines with context (document name,
//! line number, text). It enables agents to discover relevant information without
//! reading entire documents.
//!
//! **Integration points:**
//! - Searches all documents in compile-time `DOCS` constant
//! - Emits streaming `Frame::item` for each match (up to limit)
//! - Terminates with `Frame::ok` containing total match count
//!
//! **Frame protocol:**
//! - Emits zero or more `Frame::item` with `{doc, line, text}` for matches
//! - Final `Frame::ok` with `{count}` indicating total results
//! - Respects configurable result limit (default 20, max 50)
//!
//! **Search algorithm:**
//! - Case-insensitive substring matching (query.to_lowercase().contains())
//! - Line-by-line scanning across all documents
//! - First N matches returned (early termination at limit)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Simple substring search**: Easy to understand, no complex query syntax
//! - **Line-level granularity**: Returns context (document, line number) for each match
//! - **Result streaming**: Emits matches as found, not buffered in memory
//! - **Bounded results**: Limit prevents overwhelming agents with thousands of matches
//! - **Universal access**: No actor restrictions - all agents can search
//!
//! SECURITY MODEL
//! ==============
//! - **No actor restrictions**: All agents (head, hand, room) may search documentation
//! - **Read-only operation**: Cannot modify documentation or search index
//! - **Bounded output**: Result limit (max 50) prevents resource exhaustion
//! - **No filesystem access**: Searches embedded binary data only
//! - **Query validation**: Rejects empty queries to prevent degenerate behavior
//!
//! PERFORMANCE
//! ===========
//! - Linear scan O(n*m) where n=total lines, m=avg line length
//! - Acceptable for small corpus (<500KB, <10 documents)
//! - Early termination at result limit reduces wasted work
//! - No indexing overhead or memory allocation for search structures
//!
//! TRADE-OFFS
//! ==========
//! 1. **Substring vs. Full-Text Indexing**
//!    - CHOSEN: Simple substring matching with case-insensitive comparison
//!    - REJECTED: Inverted index, TF-IDF, or regex matching
//!    - WHY: Documentation corpus is small (<500KB), indexing overhead not justified
//!    - IMPLICATION: Search is O(n) in content size, but fast enough in practice
//!
//! 2. **Line vs. Context Windows**
//!    - CHOSEN: Return individual matching lines
//!    - REJECTED: Return N lines before/after match (context window)
//!    - WHY: Simpler implementation, agents can use `docs:read` for full context
//!    - IMPLICATION: Agents may need follow-up `docs:read` for surrounding context
//!
//! 3. **Result Ordering**
//!    - CHOSEN: Document order + line number order (deterministic)
//!    - REJECTED: Relevance scoring or frequency-based ranking
//!    - WHY: Deterministic results, no need for relevance algorithm on small corpus
//!    - IMPLICATION: High-quality matches may appear late in results

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

use super::DOCS;

// =============================================================================
// CONSTANTS
// =============================================================================

/// Default maximum number of search results.
///
/// WHY: Prevents overwhelming agents with thousands of matches from common
/// terms like "agent" or "kernel". 20 results balance discoverability with
/// information overload.
const DEFAULT_LIMIT: u64 = 20;

/// Maximum allowed search result limit.
///
/// WHY: Hard cap prevents resource exhaustion from malicious or buggy agents
/// requesting excessive results. 50 results is sufficient for most use cases.
const MAX_LIMIT: u64 = 50;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for searching documentation content.
///
/// WHY: Enables agents to discover relevant documentation without reading
/// entire files. Useful for finding specific topics, API references, or
/// examples across all embedded documentation.
pub struct DocsSearch;

impl Default for DocsSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl DocsSearch {
    /// Create a new `DocsSearch` syscall.
    ///
    /// WHY: Zero-config constructor - all state is in the shared `DOCS` constant.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for DocsSearch {
    fn name(&self) -> &'static str {
        "docs:search"
    }

    /// Search all documentation for lines matching a query string.
    ///
    /// WHY: Provides full-text search across embedded documentation, enabling
    /// agents to discover relevant information without reading entire documents.
    /// Particularly useful for finding syscall examples, architecture concepts,
    /// or tool usage patterns.
    ///
    /// USE CASE: Invoked by all agent types when searching for specific topics:
    /// - Head agents searching for "security" to understand permission model
    /// - Hand agents searching for "tool" to find LLM tool specifications
    /// - Room agents searching for "concurrency" to understand task lanes
    ///
    /// ARGUMENTS:
    /// - `query` (string, required) - Case-insensitive substring to search for
    /// - `limit` (integer, optional) - Max results to return (default 20, max 50)
    ///
    /// RETURNS:
    /// - Zero or more `Frame::item` with `{doc, line, text}` for each match
    /// - Final `Frame::ok` with `{count}` indicating total results returned
    /// - `E_INVALID_ARGS` if query is missing or empty
    ///
    /// ALGORITHM: Simple case-insensitive substring matching with early termination
    /// at result limit. Searches documents in catalog order, emitting matches as found.
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Cancellation Check
        // =====================================================================
        // WHY: Respect context cancellation before performing potentially
        // expensive search operation. Prevents wasted work on cancelled tasks.
        ctx.check_cancelled()?;

        // =====================================================================
        // PHASE 2: Argument Extraction & Validation
        // =====================================================================
        // WHY: Extract and validate search query and result limit. Trimming
        // whitespace prevents spurious empty query errors. Limit clamping
        // prevents resource exhaustion from excessive result requests.
        let query = data
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if query.is_empty() {
            return Err(KernelError::invalid_args("query is required"));
        }

        // WHY: Clamp limit between 1 and MAX_LIMIT (50). This prevents both
        // zero-result requests (useless) and unbounded result requests (DoS).
        let limit = data
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT) as usize;

        // WHY: Convert query to lowercase once, outside the loop, for efficiency.
        // Case-insensitive matching without repeated allocations.
        let query_lower = query.to_lowercase();

        // =====================================================================
        // PHASE 3: Search Execution with Streaming Results
        // =====================================================================
        // WHY: Stream results as we find them, rather than buffering in memory.
        // This provides faster time-to-first-result and bounds memory usage.
        //
        // ALGORITHM: Double-nested loop with early termination:
        // - Outer loop: Iterate documents in catalog order
        // - Inner loop: Scan lines within each document
        // - Break both loops when limit reached
        //
        // PERFORMANCE: O(n*m) where n=total lines, m=avg line length. Early
        // termination at limit prevents wasted work for common queries.
        let mut count: usize = 0;

        for doc in DOCS {
            if count >= limit {
                break; // WHY: Stop scanning documents when limit reached
            }

            for (line_num, line) in doc.content.lines().enumerate() {
                if count >= limit {
                    break; // WHY: Stop scanning lines when limit reached
                }

                // WHY: Case-insensitive substring matching. We convert line to
                // lowercase for comparison, avoiding repeated query lowercasing.
                if line.to_lowercase().contains(&query_lower) {
                    // WHY: Emit Frame::item immediately when match found (streaming).
                    // Line numbers are 1-indexed for human readability.
                    let _ = tx
                        .send(Frame::item(
                            ctx.call_id,
                            json!({
                                "doc": doc.name,
                                "line": line_num + 1,
                                "text": line,
                            }),
                        ))
                        .await;
                    count += 1;
                }
            }
        }

        // =====================================================================
        // PHASE 4: Finalization
        // =====================================================================
        // WHY: Emit final Frame::ok with total result count. This signals end
        // of stream and allows clients to report "X results found" to users.
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({ "count": count })))
            .await;

        Ok(())
    }
}
