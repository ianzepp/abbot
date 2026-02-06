//! Config:Read - Query workspace configuration with TOML parsing
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides read-only access to the workspace configuration file
//! (`.abbot/config.toml`) within the Abbot kernel's security model. It supports
//! hierarchical queries: entire config, section-level, or key-level access.
//!
//! **Configuration location:**
//! - File path: `<workspace_root>/.abbot/config.toml`
//! - Resolution: Via `runtime::workspace_config_from_root(&ctx.cwd)`
//! - Format: TOML (Tom's Obvious, Minimal Language)
//!
//! **Query hierarchy:**
//! 1. No arguments: Returns entire config as JSON-serialized TOML table
//! 2. Section only: Returns entire section (e.g., `[head]` section)
//! 3. Section + key: Returns specific key within section (e.g., `head.temperature`)
//!
//! **Integration points:**
//! - `runtime::workspace_config_from_root` for path resolution
//! - `runtime::read_optional_file` for graceful handling of missing files
//! - `SyscallContext` for cancellation and working directory context
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with entire config, section, or key value
//! - Returns `KernelError` for TOML parse errors or invalid argument combinations
//!
//! SECURITY MODEL
//! ==============
//! This syscall is **intentionally read-only** to allow safe access by all agent types:
//!
//! 1. **No Mutation Guard**
//!    - WHY: Configuration reading is inherently safe (no side effects)
//!    - IMPLICATION: "Hand", "head", and "room" agents may all read config
//!    - RATIONALE: Enables agents to adapt behavior based on workspace settings
//!      (e.g., temperature, fever mode, persona dials)
//!
//! 2. **Workspace Isolation**
//!    - WHY: Config path is resolved from `ctx.cwd` (working directory)
//!    - HOW: Each workspace has independent `.abbot/config.toml` file
//!    - ATTACK PREVENTED: Agents cannot read config from other workspaces
//!
//! 3. **TOML Parse Safety**
//!    - WHY: Malformed TOML returns error (does not crash)
//!    - HOW: `str::parse()` with error handling at line 58
//!    - IMPLICATION: Users can manually edit config without kernel crashes
//!
//! 4. **Graceful Missing File Handling**
//!    - WHY: Empty config is valid (returns empty table, not error)
//!    - HOW: `read_optional_file` returns `Ok(None)` for missing files
//!    - RATIONALE: First-time workspace usage should not require config file
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **File-based configuration**: Human-editable TOML files over database storage
//! - **Graceful degradation**: Missing config or sections return empty values (not errors)
//! - **Hierarchical queries**: Support both broad (entire config) and narrow (single key) queries
//! - **TOML format choice**: Readable, well-structured, supports comments and sections
//! - **No caching**: Always reads from disk to reflect manual edits immediately
//!
//! WHY TOML OVER JSON/YAML:
//! - More human-readable than JSON (comments, no quotes on keys)
//! - Simpler than YAML (no indentation sensitivity, fewer gotchas)
//! - Standard for Rust projects (cargo.toml, rustfmt.toml)
//! - Strong typing (distinguishes integers, floats, strings, booleans)
//!
//! PERFORMANCE
//! ===========
//! - File I/O on every read (no caching) - acceptable for infrequent config queries
//! - TOML parsing is ~10x slower than JSON, but config files are small (<1KB typically)
//! - No blocking: async syscall does not block kernel event loop
//!
//! CONCURRENCY
//! ===========
//! - No file locking required (read-only operation)
//! - Safe for concurrent reads across multiple task lanes
//! - Race condition with config:update: Reader may see partial write (mitigated by atomic_write_file_0600)
//!
//! TRADE-OFFS
//! ==========
//! 1. **No Caching**
//!    - CHOSEN: Always read from disk
//!    - WHY: Reflects manual edits immediately, simple implementation
//!    - IMPLICATION: Slower than cached reads, but config queries are infrequent
//!
//! 2. **File-Based Storage**
//!    - CHOSEN: TOML files in `.abbot/config.toml`
//!    - REJECTED: SQLite database or in-memory storage
//!    - WHY: Human-editable, version-controllable, no schema migrations
//!    - IMPLICATION: No ACID guarantees, but acceptable for config data
//!
//! 3. **Graceful Empty Config**
//!    - CHOSEN: Missing config returns empty table (not error)
//!    - WHY: First-time workspace usage should "just work"
//!    - IMPLICATION: Cannot distinguish "no config" from "empty config"

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `config:read` syscall.
///
/// WHY: Supports hierarchical queries via optional section/key parameters.
/// Query patterns:
/// - `{}`: Entire config
/// - `{section: "head"}`: Entire `[head]` section
/// - `{section: "head", key: "temperature"}`: Single `head.temperature` value
#[derive(Debug, Deserialize)]
struct ConfigReadArgs {
    /// Section name (e.g., "head", "hand", "tars").
    ///
    /// WHY: Optional to support full-config queries. TOML sections map to table keys.
    #[serde(default)]
    section: Option<String>,

    /// Key within section (e.g., "temperature", "max_tokens").
    ///
    /// WHY: Optional to support section-level queries. Requires `section` if specified.
    #[serde(default)]
    key: Option<String>,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for reading workspace configuration from TOML files.
///
/// WHY: Stateless syscall - no internal state beyond the Syscall trait implementation.
/// Configuration path is resolved per-request from syscall context working directory.
pub struct ConfigRead;

impl Default for ConfigRead {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigRead {
    /// Create a new `ConfigRead` syscall.
    ///
    /// WHY: Zero-sized struct - no configuration needed for read-only operation.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ConfigRead {
    fn name(&self) -> &'static str {
        "config:read"
    }

    /// Read workspace configuration with hierarchical querying.
    ///
    /// WHY: Enables agents to adapt behavior based on workspace settings without requiring
    /// mutation permission. Configuration is read from `.abbot/config.toml` relative to
    /// working directory, supporting workspace isolation.
    ///
    /// USE CASE: Invoked by all agent types ("head", "hand", "room") to query configuration:
    /// - Head agents: Check fever mode, temperature settings
    /// - Hand agents: Retrieve max_tokens, generation mode
    /// - Room agents: Query persona dials, tick intervals
    ///
    /// QUERY PATTERNS:
    /// - `{section: null, key: null}`: Returns entire config as JSON-serialized TOML
    /// - `{section: "head", key: null}`: Returns entire `[head]` section
    /// - `{section: "head", key: "temperature"}`: Returns single `head.temperature` value
    /// - `{section: null, key: "temperature"}`: Error - key requires section
    ///
    /// RETURNS:
    /// - `Frame::ok` with entire config (no args), section (section only), or key value (section+key)
    /// - `E_INVALID_ARGS` if key specified without section or parse error
    /// - `E_IO` for file read errors (excluding missing file, which returns empty config)
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Argument Parsing
        // =====================================================================
        // WHY: Validate query structure before file I/O. Fail fast on invalid
        // argument combinations (e.g., key without section).
        ctx.check_cancelled()?;

        let args: ConfigReadArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // =====================================================================
        // PHASE 2: Config File Reading
        // =====================================================================
        // WHY: Resolve config path from working directory (workspace isolation).
        // Missing config file is graceful (returns empty config, not error).
        let config_path = crate::runtime::workspace_config_from_root(&ctx.cwd);

        // WHY: `read_optional_file` returns Ok(None) for missing files, enabling
        // first-time workspace usage without pre-creating config.toml.
        let config_str = match crate::runtime::read_optional_file(&config_path) {
            Ok(Some(s)) => s,
            Ok(None) => String::new(), // WHY: Missing file is valid (empty config)
            Err(e) => return Err(KernelError::io(format!("failed to read config: {e}"))),
        };

        // =====================================================================
        // PHASE 3: TOML Parsing
        // =====================================================================
        // WHY: Parse TOML into strongly-typed table. Empty string produces empty
        // table (not parse error).
        let config: toml::Table = if config_str.is_empty() {
            toml::Table::new() // WHY: Empty file is valid (no TOML to parse)
        } else {
            // WHY: Parse errors return E_IO (not crash). Enables manual config edits
            // without risking kernel stability.
            config_str
                .parse()
                .map_err(|e| KernelError::io(format!("invalid config TOML: {e}")))?
        };

        // =====================================================================
        // PHASE 4: Query Execution
        // =====================================================================
        // WHY: Four query patterns based on (section, key) combination.
        // Missing sections/keys return null (not error) for graceful handling.
        let result = match (args.section.as_deref(), args.key.as_deref()) {
            // WHY: No arguments = return entire config as JSON-serialized TOML.
            // Useful for debugging or bulk config export.
            (None, None) => json!(config),

            // WHY: Section only = return entire section as JSON object.
            // Missing section returns empty table (not error).
            (Some(section), None) => {
                let value = config
                    .get(section)
                    .cloned()
                    .unwrap_or(toml::Value::Table(toml::Table::new())); // WHY: Graceful missing section
                json!({ "section": section, "value": value })
            }

            // WHY: Section + key = return single value within section.
            // Missing section or key returns null (not error).
            (Some(section), Some(key)) => {
                let value = config
                    .get(section)
                    .and_then(|s| s.as_table())
                    .and_then(|t| t.get(key))
                    .cloned(); // WHY: None if section/key missing (graceful)
                json!({ "section": section, "key": key, "value": value })
            }

            // WHY: Key without section is invalid (TOML structure requires section).
            // Return error to prevent ambiguous queries.
            (None, Some(_)) => {
                return Err(KernelError::invalid_args("key requires section"));
            }
        };

        let _ = tx.send(Frame::ok(ctx.call_id, result)).await;
        Ok(())
    }
}
