//! Models Namespace - LLM model catalog and metadata operations
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides read-only access to the LLM model catalog for agents.
//! Model metadata is stored in JSON cache files under `~/.config/abbot/providers/`,
//! populated by external tools (e.g., `abbot-cli providers refresh`) that fetch
//! model information from LLM provider APIs.
//!
//! **Integration points:**
//! - Model cache directory: `~/.config/abbot/providers/*.json`
//! - Cache format: Per-provider JSON files with model arrays
//! - Provider support: Anthropic, OpenAI, OpenRouter, etc.
//! - Consumed by: LLM runtime for model selection and configuration
//!
//! **Key operations:**
//! - `models:list` - Enumerate available LLM models from all providers
//!
//! **Model metadata includes:**
//! - Model ID (e.g., "anthropic/claude-3-opus-20240229")
//! - Display name (e.g., "Claude 3 Opus")
//! - Context window size (tokens)
//! - Pricing information (input/output cost per token)
//! - Provider identifier (e.g., "anthropic", "openai", "openrouter")
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Filesystem-based catalog**: Models loaded from JSON cache files, not embedded
//! - **Provider-agnostic**: Supports multiple LLM providers with unified schema
//! - **Lazy loading**: Cache files read on-demand, not at kernel startup
//! - **Universal access**: All agents can list models without security restrictions
//! - **External refresh**: Cache population is out-of-band (not via syscalls)
//!
//! SECURITY MODEL
//! ==============
//! - **Read-only access**: No syscalls in this namespace modify cache files
//! - **No actor restrictions**: All agents (head, hand, room) can list models
//! - **Bounded catalog**: Individual provider caches typically <1MB
//! - **No network access**: Syscalls read local cache, don't fetch from APIs
//! - **Path isolation**: Cache directory is fixed (`~/.config/abbot/providers/`)
//!
//! CACHE FORMAT
//! ============
//! Per-provider JSON files with structure:
//! ```json
//! {
//!   "provider": "anthropic",
//!   "fetched_at": "2025-01-15T10:30:00Z",
//!   "models": [
//!     {
//!       "id": "claude-3-opus-20240229",
//!       "name": "Claude 3 Opus",
//!       "context_window": 200000,
//!       "input_cost": 0.000015,
//!       "output_cost": 0.000075
//!     }
//!   ]
//! }
//! ```
//!
//! **Cache population:**
//! - Managed by external tools (e.g., `abbot-cli providers refresh`)
//! - Fetches model metadata from provider APIs
//! - Updates are manual or scheduled (not automatic)
//!
//! PERFORMANCE
//! ===========
//! - Cache files read from disk on `models:list` invocation
//! - No caching in memory (intentional: allows external tools to update files)
//! - Typical catalog size: 10-200 models across all providers (<500KB total)
//! - JSON parsing overhead is acceptable for infrequent operations
//!
//! TRADE-OFFS
//! ==========
//! 1. **Filesystem vs. Embedded Catalog**
//!    - CHOSEN: Filesystem-based cache with external refresh
//!    - REJECTED: Embedded model list or in-memory cache
//!    - WHY: Model catalogs change frequently (new models, pricing updates)
//!    - IMPLICATION: Models:list may fail if cache files don't exist
//!
//! 2. **On-Demand vs. Startup Loading**
//!    - CHOSEN: Read cache files on-demand during `models:list`
//!    - REJECTED: Load all models into memory at kernel startup
//!    - WHY: Reduces kernel startup time, avoids stale cache on long-running processes
//!    - IMPLICATION: First `models:list` call incurs disk I/O latency
//!
//! 3. **Provider Multiplexing**
//!    - CHOSEN: Unified model ID format "provider/model-id"
//!    - REJECTED: Separate namespaces per provider
//!    - WHY: Allows LLM runtime to route requests without additional lookups
//!    - IMPLICATION: Model IDs must include provider prefix for disambiguation

mod list;

pub use list::ModelsList;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

// =============================================================================
// REGISTRATION
// =============================================================================

/// Register all model catalog syscalls with the kernel dispatcher.
///
/// WHY: Single registration point for all models namespace syscalls. Called
/// during kernel initialization to populate the syscall routing table.
///
/// SYSCALLS REGISTERED:
/// - `models:list` - Enumerate available LLM models from provider cache files
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(ModelsList::new()));
}
