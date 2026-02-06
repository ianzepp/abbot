//! Models:List - Enumerate available LLM models from provider caches
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall reads LLM model metadata from filesystem-based provider cache files
//! located in `~/.config/abbot/providers/`. Each provider (Anthropic, OpenAI, OpenRouter)
//! has a separate JSON file containing model catalog information fetched from their APIs.
//!
//! **Integration points:**
//! - Cache directory: `~/.config/abbot/providers/*.json`
//! - Cache population: External tools (e.g., `abbot-cli providers refresh`)
//! - Consumed by: LLM runtime for model selection and configuration
//! - Used by: All agents during model selection or capability discovery
//!
//! **Frame protocol:**
//! - Emits single `Frame::ok` with array of model metadata objects
//! - Returns empty array if cache directory doesn't exist
//! - Silently skips invalid or malformed cache files
//!
//! **Model ID format:**
//! - Unified format: `provider/model-id` (e.g., "anthropic/claude-3-opus-20240229")
//! - OpenRouter special case: Preserves existing slashes (e.g., "openrouter/anthropic/claude-3")
//! - Ensures globally unique identifiers for LLM runtime routing
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Lazy loading**: Read cache files on-demand, not at kernel startup
//! - **Best-effort**: Silently skip malformed files, return partial results
//! - **Provider-agnostic**: Unified schema across all LLM providers
//! - **Universal access**: No actor restrictions - all agents can list models
//! - **No network I/O**: Only reads local cache, never fetches from APIs
//!
//! SECURITY MODEL
//! ==============
//! - **No actor restrictions**: All agents (head, hand, room) may list models
//! - **Read-only operation**: Cannot modify provider cache files
//! - **Path isolation**: Only reads from `~/.config/abbot/providers/` (no arbitrary paths)
//! - **Bounded output**: Typical catalog is <500KB (10-200 models across providers)
//! - **No code execution**: Only JSON deserialization (no eval or shell commands)
//!
//! PERFORMANCE
//! ===========
//! - Disk I/O: Reads all provider cache files on each invocation
//! - JSON parsing: O(n) where n is total cache file size (typically <500KB)
//! - No caching: Intentionally reads filesystem each time to detect external updates
//! - Memory: Single allocation for result vector (bounded by provider catalog size)
//!
//! TRADE-OFFS
//! ==========
//! 1. **On-Demand vs. Cached**
//!    - CHOSEN: Read filesystem on every `models:list` invocation
//!    - REJECTED: In-memory cache with invalidation strategy
//!    - WHY: Allows external tools to update cache files dynamically
//!    - IMPLICATION: Repeated calls incur disk I/O overhead (acceptable for infrequent operations)
//!
//! 2. **Strict vs. Best-Effort Parsing**
//!    - CHOSEN: Silently skip malformed cache files, return partial results
//!    - REJECTED: Fail entire operation on first parse error
//!    - WHY: Partial model catalog is better than no catalog (one bad file doesn't break everything)
//!    - IMPLICATION: Agents may see incomplete model list if some caches are corrupt
//!
//! 3. **Model ID Format**
//!    - CHOSEN: Unified "provider/model-id" format with special OpenRouter handling
//!    - REJECTED: Provider-specific ID formats or separate namespaces
//!    - WHY: Enables LLM runtime to route requests without additional lookups
//!    - IMPLICATION: OpenRouter models preserve existing slashes (e.g., "openrouter/anthropic/claude")

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

// =============================================================================
// CACHE FILE SCHEMA
// =============================================================================
//
// Provider cache files follow a standard schema with provider identifier,
// fetch timestamp, and model array. This schema is populated by external
// tools that query provider APIs for model metadata.

/// Provider cache file schema.
///
/// WHY: Represents the structure of per-provider JSON cache files in
/// `~/.config/abbot/providers/`. Each file contains metadata for all
/// models offered by a specific provider.
///
/// POPULATED BY: External tools (e.g., `abbot-cli providers refresh`)
/// that fetch model information from provider APIs.
#[derive(Deserialize)]
struct ProviderCache {
    /// Provider identifier (e.g., "anthropic", "openai", "openrouter").
    ///
    /// WHY: Used to construct unified model IDs in "provider/model-id" format.
    provider: String,

    /// Timestamp when cache was last fetched from provider API.
    ///
    /// WHY: Allows external tools to determine cache freshness. Not currently
    /// used by this syscall, but preserved for future cache invalidation logic.
    #[allow(dead_code)]
    fetched_at: String,

    /// Array of model metadata entries.
    ///
    /// WHY: Contains the actual model catalog information (IDs, names, pricing).
    models: Vec<CachedModel>,
}

/// Individual model metadata entry.
///
/// WHY: Represents a single LLM model's metadata from a provider cache file.
/// All fields except `id` are optional to handle varying provider schemas.
#[derive(Deserialize)]
struct CachedModel {
    /// Provider-specific model identifier.
    ///
    /// WHY: Combined with provider name to create globally unique model ID.
    /// May contain slashes (e.g., OpenRouter uses "anthropic/claude-3-opus").
    id: String,

    /// Human-readable model name.
    ///
    /// WHY: Display name for UI/logging (e.g., "Claude 3 Opus" vs "claude-3-opus-20240229").
    name: Option<String>,

    /// Maximum context window size in tokens.
    ///
    /// WHY: Helps agents select appropriate models for tasks requiring large contexts.
    context_window: Option<u64>,

    /// Input token cost in dollars per token.
    ///
    /// WHY: Enables cost-aware model selection (e.g., prefer cheaper models for simple tasks).
    #[serde(default)]
    input_cost: Option<f64>,

    /// Output token cost in dollars per token.
    ///
    /// WHY: Output tokens are typically more expensive than input (generation overhead).
    #[serde(default)]
    output_cost: Option<f64>,
}

// =============================================================================
// HELPERS
// =============================================================================

/// Get the provider cache directory path.
///
/// WHY: Centralizes the logic for locating provider cache files. Uses standard
/// XDG config directory on Unix-like systems, AppData on Windows.
///
/// RETURNS: `Some(path)` if home directory exists, `None` otherwise.
///
/// PATH: `~/.config/abbot/providers/` on Unix, `%APPDATA%\abbot\providers\` on Windows.
fn providers_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("abbot").join("providers"))
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for listing available LLM models from provider caches.
///
/// WHY: Provides agents with catalog of available LLM models for selection
/// and configuration. Enables cost-aware and capability-aware model selection
/// (e.g., prefer Claude Opus for complex reasoning, Haiku for simple tasks).
pub struct ModelsList;

impl ModelsList {
    /// Create a new `ModelsList` syscall.
    ///
    /// WHY: Zero-config constructor - cache directory is determined at runtime
    /// via `providers_dir()` based on home directory.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ModelsList {
    fn name(&self) -> &'static str {
        "models:list"
    }

    /// List all available LLM models from provider cache files.
    ///
    /// WHY: Enables agents to discover available LLM models for selection and
    /// configuration. Agents typically call this when:
    /// - Selecting a model for a specific task (based on context window or cost)
    /// - Validating that a requested model exists
    /// - Displaying available models to users
    ///
    /// USE CASE: Invoked by all agent types (head, hand, room) during:
    /// - Initial model selection before spawning LLM conversations
    /// - Dynamic model switching based on task complexity
    /// - Cost estimation for multi-turn conversations
    ///
    /// ARGUMENTS: None required - listing is unconditional.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{models: [{id, name, provider, context_window, input_cost, output_cost}, ...]}`
    /// - Returns empty array if cache directory doesn't exist or contains no valid files
    /// - Never fails (best-effort parsing, silently skips malformed files)
    ///
    /// ALGORITHM: Scan provider cache directory, parse JSON files, merge model arrays.
    /// Silently skips non-JSON files, unreadable files, and malformed JSON.
    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Cancellation Check
        // =====================================================================
        // WHY: Respect context cancellation before performing filesystem I/O.
        // Prevents wasted disk reads on cancelled tasks.
        ctx.check_cancelled()?;

        // =====================================================================
        // PHASE 2: Load and Parse Provider Cache Files
        // =====================================================================
        // WHY: Read all provider cache files from `~/.config/abbot/providers/`,
        // parse JSON, and merge into unified model catalog. Best-effort parsing
        // ensures partial results even if some files are malformed.
        //
        // PERFORMANCE: O(n*m) where n=number of cache files, m=avg file size.
        // Typical case: 3-5 providers, 50-200KB per file, <500KB total.
        //
        // ERROR HANDLING: Silently skip failures at each stage:
        // - Missing cache directory -> return empty array
        // - Unreadable files -> skip file
        // - Invalid JSON -> skip file
        // - Missing required fields -> skip file
        let mut out: Vec<serde_json::Value> = Vec::new();

        if let Some(dir) = providers_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();

                    // WHY: Only process .json files, skip other file types
                    // (e.g., .bak, .tmp, README, etc.).
                    if path.extension().and_then(|s| s.to_str()) != Some("json") {
                        continue;
                    }

                    // WHY: Read file contents. Skip if unreadable (permissions,
                    // file disappeared, etc.).
                    let Ok(raw) = std::fs::read_to_string(&path) else {
                        continue;
                    };

                    // WHY: Parse JSON into ProviderCache struct. Skip if malformed
                    // or missing required fields.
                    let Ok(cache) = serde_json::from_str::<ProviderCache>(&raw) else {
                        continue;
                    };

                    // =========================================================
                    // PHASE 3: Model ID Construction and Result Accumulation
                    // =========================================================
                    // WHY: Transform provider-specific model IDs into unified
                    // "provider/model-id" format. OpenRouter requires special
                    // handling because its model IDs already contain slashes.
                    for m in cache.models {
                        // WHY: OpenRouter model IDs already contain provider
                        // prefix (e.g., "anthropic/claude-3-opus"). Other providers
                        // need explicit prefix (e.g., "anthropic" + "claude-3-opus").
                        //
                        // NORMALIZATION: Trim leading/trailing slashes to prevent
                        // malformed IDs like "provider//model-id".
                        let id = match cache.provider.as_str() {
                            "openrouter" => format!("openrouter/{}", m.id.trim_matches('/')),
                            p => format!("{}/{}", p, m.id.trim_matches('/')),
                        };

                        // WHY: Include all available metadata in response. Optional
                        // fields (name, context_window, costs) are preserved as
                        // null if missing, allowing clients to handle gracefully.
                        out.push(json!({
                            "id": id,
                            "name": m.name,
                            "provider": cache.provider,
                            "context_window": m.context_window,
                            "input_cost": m.input_cost,
                            "output_cost": m.output_cost,
                        }));
                    }
                }
            }
        }

        // =====================================================================
        // PHASE 4: Send Response
        // =====================================================================
        // WHY: Emit single Frame::ok with complete model catalog. No streaming
        // needed since catalog is reasonably sized (<500KB typical).
        //
        // NOTE: Returns empty array if cache directory doesn't exist or contains
        // no valid files. This is intentional (best-effort), not an error.
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"models": out})))
            .await;

        Ok(())
    }
}
