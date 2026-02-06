//! Config:Update - Modify workspace configuration with allowlist validation
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides controlled write access to the workspace configuration file
//! (`.abbot/config.toml`) within the Abbot kernel's security model. It enforces a
//! **strict allowlist** of section.key combinations to prevent misconfiguration or
//! unauthorized settings changes.
//!
//! **Configuration location:**
//! - File path: `<workspace_root>/.abbot/config.toml`
//! - Resolution: Via `runtime::workspace_config_from_root(&ctx.cwd)`
//! - Format: TOML (Tom's Obvious, Minimal Language)
//!
//! **Update semantics:**
//! - Atomic writes via `atomic_write_file_0600` (0600 permissions = owner-only)
//! - Read-modify-write cycle (no partial updates)
//! - Null values delete keys (rather than storing null)
//! - Missing sections are created automatically
//!
//! **Integration points:**
//! - `runtime::workspace_config_from_root` for path resolution
//! - `runtime::atomic_write_file_0600` for crash-safe writes
//! - `SyscallContext` for mutation permission and cancellation
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with updated section/key/value/status on success
//! - Returns `KernelError` for forbidden keys, invalid values, or I/O failures
//!
//! SECURITY MODEL
//! ==============
//! This syscall implements **defense-in-depth** for configuration modifications:
//!
//! 1. **Mutation Guard Requirement**
//!    - WHY: Config changes affect runtime behavior (temperature, max_tokens, etc.)
//!    - HOW: `ctx.require_mutation()` enforces "head" actor requirement at line 178
//!    - ATTACK PREVENTED: Compromised "hand" agents cannot modify agent behavior
//!    - RATIONALE: Only user-controlled "head" agents should tune configuration
//!
//! 2. **Allowlist-Based Key Validation**
//!    - WHY: Prevents arbitrary key writes that could break kernel behavior
//!    - HOW: `allowed(section, key)` checks against predefined list (line 189)
//!    - ALLOWED SECTIONS: harness, head, hand, mind, tars
//!    - ALLOWED KEYS: temperature, max_tokens, fever, persona dials, etc.
//!    - ATTACK PREVENTED: Writing malicious keys like `harness.exec_command` or `head.api_key`
//!
//! 3. **Scalar-Only Values**
//!    - WHY: Config values should be simple types (string/number/bool), not nested structures
//!    - HOW: Validation at line 197 rejects arrays and objects
//!    - RATIONALE: Prevents complex config nesting that complicates parsing/validation
//!    - IMPLICATION: Cannot store arrays like `["value1", "value2"]` via this syscall
//!
//! 4. **Atomic Writes**
//!    - WHY: Prevents partial config corruption if process crashes during write
//!    - HOW: `atomic_write_file_0600` writes to temp file, then renames atomically
//!    - ATTACK PREVENTED: Race conditions between concurrent config:read and config:update
//!    - TRADE-OFF: Readers may see old or new config, but never corrupted partial writes
//!
//! 5. **Owner-Only Permissions (0600)**
//!    - WHY: Config may contain sensitive values (API keys, tokens, etc.)
//!    - HOW: File permissions set to read/write for owner only (not group/world)
//!    - ATTACK PREVENTED: Other users on multi-user system cannot read config
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Allowlist over blocklist**: Only explicitly approved keys are writable
//! - **Scalar values only**: Simplifies validation and prevents nested config complexity
//! - **Null deletes keys**: Ergonomic API for removing config entries
//! - **Atomic writes**: Crash safety via temp file + rename pattern
//! - **File-based storage**: Human-editable, version-controllable, no schema migrations
//!
//! WHY ALLOWLIST IS RESTRICTIVE:
//! - Prevents accidental or malicious writes to undocumented keys
//! - Forces explicit approval for new config options (discoverability)
//! - Protects against typos (writing `temprature` instead of `temperature`)
//! - Enables runtime validation (we know all valid keys and their types)
//!
//! PERFORMANCE
//! ===========
//! - Read-modify-write cycle: Two file I/O operations (read + write)
//! - TOML serialization: ~5x slower than JSON, but config updates are infrequent
//! - Atomic write: Extra rename syscall, but negligible overhead for small files
//! - No caching: Subsequent reads will reflect updates immediately
//!
//! CONCURRENCY
//! ===========
//! - Mutation guard serializes updates (only one "head" agent active at a time)
//! - Atomic write prevents partial reads during concurrent config:read
//! - Last-write-wins semantics: No conflict resolution or versioning
//! - File locking: Not used (atomic rename provides sufficient guarantees)
//!
//! TRADE-OFFS
//! ==========
//! 1. **Allowlist Maintenance**
//!    - CHOSEN: Hardcoded allowlist in `allowed()` function
//!    - REJECTED: Dynamic allowlist from database or config file
//!    - WHY: Compile-time validation ensures typos are caught early
//!    - IMPLICATION: Adding new config keys requires code changes
//!
//! 2. **Scalar-Only Values**
//!    - CHOSEN: Reject arrays and objects
//!    - WHY: Simplifies validation and prevents nested config complexity
//!    - IMPLICATION: Cannot store structured data like `tags: ["rust", "cli"]`
//!
//! 3. **Last-Write-Wins**
//!    - CHOSEN: No conflict resolution or versioning
//!    - REJECTED: Optimistic locking with version numbers
//!    - WHY: Config updates are infrequent and low-conflict
//!    - IMPLICATION: Concurrent updates may silently overwrite each other

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `config:update` syscall.
///
/// WHY: Required section and key (unlike config:read) to enforce explicit updates.
/// Null values delete keys (ergonomic removal without separate delete operation).
#[derive(Debug, Deserialize)]
struct ConfigUpdateArgs {
    /// Section name (e.g., "head", "hand", "tars").
    ///
    /// WHY: Required to prevent ambiguous updates. TOML sections map to table keys.
    section: String,

    /// Key within section (e.g., "temperature", "max_tokens").
    ///
    /// WHY: Required to target specific config value. Validated against allowlist.
    key: String,

    /// New value for the key (string, number, bool, or null).
    ///
    /// WHY: JSON value enables flexible typing (integers, floats, strings, booleans).
    /// Null values trigger key deletion (removes key from section).
    value: serde_json::Value,
}

// =============================================================================
// VALIDATION HELPERS
// =============================================================================

/// Check if a section.key combination is allowed for updates.
///
/// WHY: Allowlist-based validation prevents arbitrary config writes. Only explicitly
/// approved keys may be modified, protecting against misconfiguration or attacks.
///
/// SECTIONS:
/// - `harness`: Test framework settings (slow_idle, deep_idle)
/// - `head`: Head agent config (temperature, max_tokens, fever, generation mode)
/// - `hand`: Hand agent config (temperature, max_iters, output limits)
/// - `mind`: Room/Mind agent config (temperature, tact, tick_interval)
/// - `tars`: Personality dials (humor, honesty, sarcasm, empathy, etc.)
///
/// SECURITY: Returns false for unknown sections or keys, preventing unauthorized writes.
fn allowed(section: &str, key: &str) -> bool {
    match section {
        // WHY: Harness settings control test execution speed (idle delays)
        "harness" => matches!(key, "slow_idle" | "deep_idle"),

        // WHY: Head agent settings control decision-making behavior
        "head" => matches!(
            key,
            "temperature"
                | "max_tokens"
                | "fever"
                | "generation"
                | "autist"
                | "heartbeat_tick"
                | "debounce_ms"
                | "time_gap_marker_minutes"
        ),

        // WHY: Hand agent settings control tool execution behavior
        "hand" => matches!(
            key,
            "temperature"
                | "max_tokens"
                | "fever"
                | "generation"
                | "autist"
                | "max_iters"
                | "max_output_chars_in_prompt"
                | "max_trace_entries_in_prompt"
        ),

        // WHY: Mind/Room agent settings control reflection and coordination
        "mind" => matches!(
            key,
            "temperature"
                | "max_tokens"
                | "fever"
                | "generation"
                | "autist"
                | "tact"
                | "tick_interval"
        ),

        // WHY: TARS personality dials control agent persona and communication style
        "tars" => matches!(
            key,
            "humor"
                | "honesty"
                | "sarcasm"
                | "verbosity"
                | "confidence"
                | "curiosity"
                | "patience"
                | "formality"
                | "empathy"
                | "pedantry"
                | "initiative"
                | "optimism"
                | "caution"
        ),

        // WHY: Unknown sections are forbidden by default (allowlist-based security)
        _ => false,
    }
}

/// Convert JSON value to TOML value with type preservation.
///
/// WHY: Syscall arguments are JSON (serde_json), but config file is TOML.
/// This conversion preserves numeric types (integer vs. float) and handles
/// nested structures (though scalar-only validation prevents their use).
///
/// TYPE MAPPING:
/// - JSON null → TOML string (empty) - used for deletion sentinel
/// - JSON bool → TOML boolean
/// - JSON number → TOML integer or float (auto-detected)
/// - JSON string → TOML string
/// - JSON array → TOML array (rejected by scalar validation)
/// - JSON object → TOML table (rejected by scalar validation)
fn json_to_toml(v: &serde_json::Value) -> toml::Value {
    match v {
        // WHY: Null values are used for deletion, not storage. Converted to empty
        // string for type safety (TOML has no native null).
        serde_json::Value::Null => toml::Value::String(String::new()),

        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),

        // WHY: Preserve numeric types. Integers stay integers (e.g., max_tokens=1000),
        // floats stay floats (e.g., temperature=0.7).
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                // WHY: Fallback for exotic number types (u64 > i64::MAX)
                toml::Value::String(n.to_string())
            }
        }

        serde_json::Value::String(s) => toml::Value::String(s.clone()),

        // WHY: Recursive conversion for arrays (rejected by scalar validation in practice)
        serde_json::Value::Array(arr) => {
            toml::Value::Array(arr.iter().map(json_to_toml).collect())
        }

        // WHY: Recursive conversion for objects (rejected by scalar validation in practice)
        serde_json::Value::Object(obj) => {
            let mut table = toml::Table::new();
            for (k, val) in obj {
                table.insert(k.clone(), json_to_toml(val));
            }
            toml::Value::Table(table)
        }
    }
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for updating workspace configuration with allowlist validation.
///
/// WHY: Stateless syscall - no internal state beyond the Syscall trait implementation.
/// Configuration path is resolved per-request from syscall context working directory.
pub struct ConfigUpdate;

impl ConfigUpdate {
    /// Create a new `ConfigUpdate` syscall.
    ///
    /// WHY: Zero-sized struct - allowlist is hardcoded in `allowed()` function.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for ConfigUpdate {
    fn name(&self) -> &'static str {
        "config:update"
    }

    /// Update workspace configuration with allowlist validation and atomic writes.
    ///
    /// WHY: Enables controlled configuration changes by "head" agents while preventing
    /// unauthorized or malicious config modifications. Uses read-modify-write cycle with
    /// atomic file writes for crash safety.
    ///
    /// USE CASE: Invoked by "head" agents to update workspace settings:
    /// - Adjust agent behavior: temperature, max_tokens, fever mode
    /// - Tune persona: TARS personality dials (humor, formality, etc.)
    /// - Configure test harness: idle delays for test execution
    ///
    /// UPDATE SEMANTICS:
    /// - `value != null`: Sets `section.key = value` (creates section if missing)
    /// - `value == null`: Deletes `section.key` (removes key from section)
    /// - Empty section after deletion: Section remains in file (empty `[section]`)
    ///
    /// SECURITY NOTE: This syscall enforces multiple security layers:
    /// 1. Mutation guard - only "head" actors may update config (line 178)
    /// 2. Allowlist validation - only approved section.key combinations allowed (line 189)
    /// 3. Scalar-only values - rejects arrays and objects (line 197)
    /// 4. Atomic writes - prevents partial config corruption (line 245)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{section, key, value, status: "updated"}` on success
    /// - `E_FORBIDDEN` if actor lacks mutation permission
    /// - `E_INVALID_ARGS` if section/key not in allowlist or value is non-scalar
    /// - `E_IO` for file read/write errors or TOML parse failures
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Security Verification
        // =====================================================================
        // WHY: Ensure caller has mutation permission before expensive operations.
        // Config changes affect runtime behavior, so restrict to "head" agents.
        ctx.check_cancelled()?;

        // WHY: Only "head" agents may modify configuration. Prevents "hand" agents
        // (controlled by LLMs) from altering agent behavior if compromised.
        ctx.require_mutation()?;

        // =====================================================================
        // PHASE 2: Argument Parsing & Validation
        // =====================================================================
        // WHY: Validate arguments before file I/O to fail fast on invalid requests.
        let args: ConfigUpdateArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Normalize section and key (trim whitespace) to prevent " head " vs "head"
        // allowlist bypass via whitespace padding.
        let section = args.section.trim();
        let key = args.key.trim();
        if section.is_empty() || key.is_empty() {
            return Err(KernelError::invalid_args("section/key is empty"));
        }

        // WHY: Allowlist check before file I/O to fail fast on unauthorized keys.
        // Returns error with clear message indicating which key is forbidden.
        if !allowed(section, key) {
            return Err(KernelError::invalid_args(format!(
                "config key not writable: {}.{}",
                section, key
            )));
        }

        // WHY: Only scalar values (string/number/bool/null) are allowed. Arrays and
        // objects complicate validation and are rarely needed for config values.
        // Null is special-cased for deletion (not stored in TOML).
        match &args.value {
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {}
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                return Err(KernelError::invalid_args(
                    "value must be string/number/bool/null",
                ));
            }
        }

        // =====================================================================
        // PHASE 3: Config File Reading
        // =====================================================================
        // WHY: Read current config to perform read-modify-write cycle. Missing
        // config is graceful (starts with empty table).
        let config_path = crate::runtime::workspace_config_from_root(&ctx.cwd);

        // WHY: `read_optional_file` returns Ok(None) for missing files, enabling
        // config creation on first update (no pre-existing config.toml required).
        let config_str = match crate::runtime::read_optional_file(&config_path) {
            Ok(Some(s)) => s,
            Ok(None) => String::new(), // WHY: Missing file is valid (create new config)
            Err(e) => return Err(KernelError::io(format!("failed to read config: {e}"))),
        };

        // =====================================================================
        // PHASE 4: TOML Parsing
        // =====================================================================
        // WHY: Parse existing config into mutable table for modification.
        let mut config: toml::Table = if config_str.is_empty() {
            toml::Table::new() // WHY: Empty file starts with empty table
        } else {
            // WHY: Parse errors return E_IO (not crash). Prevents broken config
            // from making updates impossible (manual fix required).
            config_str
                .parse()
                .map_err(|e| KernelError::io(format!("invalid config TOML: {e}")))?
        };

        // =====================================================================
        // PHASE 5: Config Modification
        // =====================================================================
        // WHY: Modify in-memory config table. Creates section if missing (graceful).
        let section_table = config
            .entry(section)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut();

        // WHY: Ensure section is a table (not scalar). Prevents corruption if config
        // manually edited with `head = "value"` instead of `[head]` section.
        let Some(section_table) = section_table else {
            return Err(KernelError::invalid_args(format!(
                "section '{}' is not a table",
                args.section
            )));
        };

        // WHY: Null values delete keys (ergonomic removal), non-null values set/update.
        if args.value.is_null() {
            section_table.remove(key); // WHY: Remove key from section (deletion)
        } else {
            let toml_value = json_to_toml(&args.value);
            section_table.insert(key.to_string(), toml_value); // WHY: Insert/update key
        }

        // =====================================================================
        // PHASE 6: Atomic File Write
        // =====================================================================
        // WHY: Serialize modified config and write atomically to prevent corruption.
        let new_config_str = toml::to_string_pretty(&config).unwrap_or_default();

        // WHY: `atomic_write_file_0600` writes to temp file + rename for atomicity.
        // Prevents partial writes if process crashes. 0600 permissions restrict
        // access to owner only (config may contain sensitive values).
        if let Err(e) = crate::runtime::atomic_write_file_0600(&config_path, &new_config_str) {
            return Err(KernelError::io(format!("failed to write config: {e}")));
        }

        // WHY: Include original JSON value in response (not TOML-converted value)
        // to match input format. Status field indicates successful update.
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "section": section,
                    "key": key,
                    "value": args.value,
                    "status": "updated"
                }),
            ))
            .await;

        Ok(())
    }
}
