//! Fs:Search - Content search with VFS path validation and regex support
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides recursive content search within VFS-mounted workspaces with support
//! for literal string matching, regex patterns, case sensitivity control, and glob-based file
//! filtering. It implements result truncation and line clipping to prevent memory exhaustion.
//!
//! **Integration points:**
//! - `VfsSource` for VFS resolution (validates search root path)
//! - `walkdir` crate for recursive file traversal
//! - `regex` crate for pattern matching with case-insensitive support
//! - `globset` crate for filename filtering (e.g., only search `*.rs` files)
//! - `SyscallContext` for cancellation propagation
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{ "matches": [...], "truncated": bool }` payload
//! - Each match: `{ "path": "...", "line": N, "text": "..." }` (line 1-indexed)
//! - Returns `E_NOT_FOUND` for missing directories, `E_INVALID_ARGS` for malformed regex/glob
//!
//! SECURITY MODEL
//! ==============
//! 1. **VFS Path Resolution** (Primary Security Boundary)
//!    - WHY: Prevents searching directories outside mounted workspaces
//!    - HOW: `vfs.resolve(&path)` validates search root against mount table (line 148)
//!    - ATTACK PREVENTED: Path traversal (`../../etc`), absolute paths (`/var/log`)
//!
//! 2. **Result Limiting** (Resource Exhaustion Prevention)
//!    - WHY: Prevents memory exhaustion from massive search result sets
//!    - HOW: `max_results` parameter (default 200, cap 5000) truncates matches (line 119)
//!    - ATTACK PREVENTED: Resource exhaustion DoS from broad queries (e.g., search "a" in entire repo)
//!    - TRADE-OFF: Large result sets require multiple queries with refined patterns
//!
//! 3. **Line Clipping** (Payload Size Control)
//!    - WHY: Prevents Frame payload explosion from extremely long lines
//!    - HOW: Truncate each matched line to 400 characters (line 227)
//!    - ATTACK PREVENTED: Memory exhaustion from minified files with 10KB+ lines
//!    - TRADE-OFF: Long lines are incomplete (acceptable for preview purposes)
//!
//! 4. **Binary File Skipping**
//!    - WHY: Prevents crashes on binary content with invalid UTF-8
//!    - HOW: `read_to_string()` fails gracefully on binary files, skipped via `continue` (line 205)
//!    - ATTACK PREVENTED: Crashes or corruption from reading binary files as text
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Read-only operation**: No mutation guard (all agents may search)
//! - **Regex power with safety**: Regex compilation errors rejected early
//! - **Case-insensitive default**: `regex` flag with `(?i)` prefix for case-insensitive mode
//! - **Workspace-relative paths**: Return paths relative to mount root (not host filesystem)
//! - **Truncation transparency**: `truncated` flag indicates incomplete results
//! - **Fail-safe defaults**: 200 result limit prevents memory exhaustion
//!
//! PERFORMANCE
//! ===========
//! - Reads entire file into memory per-file (not streaming line-by-line)
//! - TRADE-OFF: Simpler implementation, acceptable for typical source files (<1MB)
//! - Regex compilation happens once before traversal (not per-line)
//! - GlobSet compilation happens once for file filtering
//! - Line clipping at 400 chars limits per-match memory usage
//!
//! TRADE-OFFS
//! ==========
//! 1. **Full File Read vs. Streaming**
//!    - CHOSEN: Read entire file into memory, then search line-by-line
//!    - WHY: Simpler implementation, typical source files fit in memory
//!    - COST: Large files (>10MB) consume memory even if match is on line 1
//!    - ACCEPTABLE: Most source files are <1MB, binary files skipped
//!
//! 2. **Result Truncation vs. Pagination**
//!    - CHOSEN: Truncate at max_results (not cursor-based pagination)
//!    - WHY: Simpler implementation, stateless operation
//!    - IMPLICATION: Callers cannot resume search (must refine query/pattern)
//!
//! 3. **Line-Based Matching vs. Multiline Regex**
//!    - CHOSEN: Match line-by-line (no multiline regex support)
//!    - WHY: Simpler implementation, matches typical search use cases
//!    - LIMITATION: Cannot match patterns spanning multiple lines

use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::vfs::MountTable;

use super::VfsSource;

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `fs:search` syscall.
///
/// WHY: Supports both literal and regex content search with file filtering.
/// Result limiting prevents memory exhaustion from broad queries.
#[derive(Debug, Deserialize)]
struct FsSearchArgs {
    /// Search query (literal string or regex pattern).
    ///
    /// WHY: Core search term. Interpreted as literal string unless `regex: true`.
    query: String,

    /// Virtual path to search root directory. Defaults to ".".
    ///
    /// WHY: VFS resolution ensures path is within mounted workspace.
    #[serde(default)]
    path: String,

    /// Glob pattern for filename filtering (e.g., "*.rs", "test_*.txt").
    ///
    /// WHY: Limits search to specific file types (improves performance and relevance).
    #[serde(default)]
    include: String,

    /// Enable regex mode for query (default: literal string).
    ///
    /// WHY: Provides regex power with explicit opt-in (prevents accidental regex syntax).
    #[serde(default)]
    regex: bool,

    /// Enable case-sensitive matching (default: case-insensitive).
    ///
    /// WHY: Most searches benefit from case-insensitive matching (e.g., "TODO" vs "todo").
    #[serde(default)]
    case_sensitive: bool,

    /// Maximum number of results to return.
    ///
    /// WHY: Prevents memory exhaustion from broad queries. Default 200, capped at 5000.
    #[serde(default)]
    max_results: Option<usize>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for recursive content search with VFS path validation.
///
/// WHY: Provides secure content search for agents while maintaining workspace
/// isolation through VFS resolution. Supports regex, case control, and file filtering.
pub struct FsSearch {
    /// VFS resolution strategy.
    ///
    /// WHY: Enables dependency injection for testing (Global, Disabled, or custom Table).
    vfs: VfsSource,
}

impl FsSearch {
    /// Create a new `FsSearch` syscall with global VFS.
    ///
    /// WHY: Standard constructor for production use with global mount table.
    pub fn new() -> Self {
        Self {
            vfs: VfsSource::Global,
        }
    }
}

impl Default for FsSearch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Syscall for FsSearch {
    fn name(&self) -> &'static str {
        "fs:search"
    }

    /// Search file contents with VFS validation and pattern matching.
    ///
    /// WHY: Provides secure content search for agents while maintaining workspace
    /// isolation. Supports literal strings, regex patterns, case sensitivity control,
    /// and glob-based file filtering.
    ///
    /// USE CASE: Invoked by agents to find code patterns, TODOs, or configuration values.
    /// Common patterns:
    /// - Find TODO comments: `{ "query": "TODO", "include": "*.rs" }`
    /// - Regex search: `{ "query": "fn \\w+\\(", "regex": true, "include": "*.rs" }`
    /// - Case-sensitive: `{ "query": "ERROR", "case_sensitive": true }`
    ///
    /// SECURITY NOTE: VFS resolution (line 148) is the trust boundary. Search root
    /// must be within mounted workspace.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{ "matches": [...], "truncated": bool }` payload
    /// - Each match: `{ "path": "...", "line": N, "text": "..." }` (line 1-indexed, text clipped to 400 chars)
    /// - `E_NOT_FOUND` if search root does not exist
    /// - `E_DISABLED` if VFS is not configured
    /// - `E_FORBIDDEN` if path is outside mounted workspace
    /// - `E_INVALID_ARGS` if query empty, regex invalid, or glob malformed
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // ---------------------------------------------------------------------
        // PHASE 1: Argument Parsing & Validation
        // ---------------------------------------------------------------------
        ctx.check_cancelled()?;

        let args: FsSearchArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let query = args.query.clone();
        if query.trim().is_empty() {
            return Err(KernelError::invalid_args("query is required"));
        }

        // WHY: Cap max_results at 5000 to prevent Frame payload explosion.
        // Default 200 is reasonable for most search queries.
        let max_results = args.max_results.unwrap_or(200).min(5000);

        // ---------------------------------------------------------------------
        // PHASE 2: VFS Path Resolution (SECURITY BOUNDARY)
        // ---------------------------------------------------------------------
        let vfs = match &self.vfs {
            VfsSource::Disabled => {
                return Err(KernelError::disabled(
                    "filesystem access disabled: no mounts configured",
                ));
            }
            VfsSource::Global => MountTable::global().ok_or_else(|| {
                KernelError::disabled("filesystem access disabled: no mounts configured")
            })?,
            VfsSource::Table(t) => t.as_ref(),
        };

        // WHY: Default to "." (current directory) if path is empty
        let path = if args.path.trim().is_empty() {
            ".".to_string()
        } else {
            args.path.clone()
        };

        // WHY: resolve() is the trust boundary - validates path against mount table
        let resolved = vfs.resolve(&path)?;
        let base = &resolved.host_path;

        if !base.exists() {
            return Err(KernelError::not_found(format!(
                "directory not found: {}",
                args.path
            )));
        }

        // ---------------------------------------------------------------------
        // PHASE 3: Glob and Regex Compilation
        // ---------------------------------------------------------------------
        // WHY: Compile glob pattern once before file traversal (not per-file).
        // Empty `include` matches all files.
        let include_set: Option<GlobSet> = if !args.include.trim().is_empty() {
            let glob = Glob::new(args.include.trim())
                .map_err(|e| KernelError::invalid_args(format!("invalid include: {e}")))?;
            let mut builder = GlobSetBuilder::new();
            builder.add(glob);
            Some(
                builder
                    .build()
                    .map_err(|e| KernelError::invalid_args(format!("invalid include: {e}")))?,
            )
        } else {
            None
        };

        // WHY: Compile regex once before search (not per-line). Add `(?i)` prefix
        // for case-insensitive mode (regex crate convention).
        let re = if args.regex {
            let pat = if args.case_sensitive {
                query.clone()
            } else {
                format!("(?i){}", query)
            };
            Some(
                Regex::new(&pat)
                    .map_err(|e| KernelError::invalid_args(format!("invalid regex: {e}")))?,
            )
        } else {
            None
        };

        // WHY: Compute workspace root for relative path stripping
        let ws_root = &resolved.mount.host_path;
        let mut matches = Vec::new();

        // ---------------------------------------------------------------------
        // PHASE 4: File Traversal & Content Search
        // ---------------------------------------------------------------------
        // WHY: Use walkdir with symlink protection (follow_links: false)
        for entry in walkdir::WalkDir::new(base)
            .follow_links(false)  // WHY: Prevent symlink loops
            .into_iter()
            .filter_map(|e| e.ok())
        {
            // WHY: Only search regular files (skip directories, symlinks)
            if !entry.file_type().is_file() {
                continue;
            }

            // WHY: Apply glob filter to filename (not full path)
            let file_name = entry.file_name().to_string_lossy();
            if let Some(inc) = &include_set {
                if !inc.is_match(file_name.as_ref()) {
                    continue;
                }
            }

            // WHY: Read entire file into memory. Skip binary files (read_to_string fails).
            let path = entry.path();
            let content = match tokio::fs::read_to_string(path).await {
                Ok(c) => c,
                Err(_) => continue,  // WHY: Skip binary files or read errors
            };

            // WHY: Search line-by-line (not multiline regex support)
            for (i, line) in content.lines().enumerate() {
                // WHY: Match using regex (if enabled) or literal string (case-sensitive or not)
                let hit = if let Some(re) = &re {
                    re.is_match(line)
                } else if args.case_sensitive {
                    line.contains(&query)
                } else {
                    line.to_ascii_lowercase()
                        .contains(&query.to_ascii_lowercase())
                };

                if hit {
                    // WHY: Strip workspace root prefix to return virtual paths
                    let rel = path
                        .strip_prefix(ws_root)
                        .ok()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());

                    // WHY: Clip long lines to 400 chars to limit Frame payload size
                    // (minified files can have 10KB+ lines)
                    let text: String = line.chars().take(400).collect();
                    matches.push(json!({
                        "path": rel,
                        "line": i + 1,  // WHY: 1-indexed line numbers (common convention)
                        "text": text
                    }));

                    // WHY: Truncate at max_results to prevent memory exhaustion
                    if matches.len() >= max_results {
                        break;
                    }
                }
            }
            if matches.len() >= max_results {
                break;
            }
        }

        // ---------------------------------------------------------------------
        // PHASE 5: Response Formatting
        // ---------------------------------------------------------------------
        let truncated = matches.len() >= max_results;
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"matches": matches, "truncated": truncated}),
            ))
            .await;

        Ok(())
    }
}
