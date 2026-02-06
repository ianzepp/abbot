//! Config Namespace - Workspace configuration management via TOML files
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides read and write access to workspace configuration stored in
//! `.abbot/config.toml` files. Configuration is **workspace-scoped** (each workspace
//! has independent config) and **file-based** (human-editable TOML rather than database).
//!
//! **Registered syscalls:**
//! - `config:read` - Query configuration with hierarchical access (entire config, section, or key)
//! - `config:update` - Modify configuration with allowlist validation and atomic writes
//!
//! **Configuration structure:**
//! ```toml
//! [harness]
//! slow_idle = 100
//! deep_idle = 500
//!
//! [head]
//! temperature = 0.7
//! max_tokens = 4096
//! fever = "high"
//! generation = "sonnet"
//!
//! [hand]
//! temperature = 0.0
//! max_iters = 10
//!
//! [mind]
//! temperature = 0.5
//! tact = "high"
//!
//! [tars]
//! humor = 80
//! honesty = 95
//! sarcasm = 30
//! ```
//!
//! **File location:**
//! - Path: `<workspace_root>/.abbot/config.toml`
//! - Resolution: Via `runtime::workspace_config_from_root(&ctx.cwd)`
//! - Permissions: 0600 (owner-only read/write)
//!
//! **Integration with runtime:**
//! - `WorkspaceConfigToml::load_from_workspace_root` reads full config
//! - `HeadBundleConfig::from_workspace` loads head agent settings
//! - `HandBundleConfig::from_workspace` loads hand agent settings
//! - `load_tars_dials` reads TARS personality configuration
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **File-based configuration**: Human-editable, version-controllable, no schema migrations
//! - **Workspace isolation**: Each workspace has independent config (no global settings)
//! - **TOML format**: Readable, structured, standard for Rust projects
//! - **Allowlist-based writes**: Only approved keys may be modified (prevents misconfiguration)
//! - **Scalar values only**: Simple types (string/number/bool) over nested structures
//! - **Graceful missing config**: Empty config is valid (sensible defaults apply)
//!
//! WHY FILE-BASED OVER DATABASE:
//! - **Human-editable**: Users can modify config with any text editor
//! - **Version-controllable**: Config changes tracked in git history
//! - **No migrations**: Adding new keys doesn't require schema updates
//! - **Portable**: Config files copy easily across machines
//! - **Debuggable**: Config is readable without special tools
//!
//! WHY TOML OVER JSON/YAML:
//! - **Comments**: TOML supports comments for documenting settings
//! - **Sections**: Natural grouping via `[section]` headers
//! - **Type-safe**: Distinguishes integers, floats, strings, booleans
//! - **Rust-standard**: Consistent with cargo.toml, rustfmt.toml
//! - **Simple**: No indentation sensitivity (YAML) or quote-heavy syntax (JSON)
//!
//! SECURITY MODEL
//! ==============
//! Configuration access follows a **read-open, write-restricted** model:
//!
//! 1. **config:read - No Mutation Guard**
//!    - WHY: Reading config is inherently safe (no side effects)
//!    - IMPLICATION: All agents ("head", "hand", "room") may read config
//!    - USE CASE: Agents adapt behavior based on workspace settings
//!
//! 2. **config:update - Mutation Guard Required**
//!    - WHY: Config changes affect runtime behavior (prevent LLM tampering)
//!    - IMPLICATION: Only "head" agents (user-controlled) may update config
//!    - USE CASE: User adjusts temperature, persona dials, generation mode
//!
//! 3. **Allowlist-Based Writes**
//!    - WHY: Prevents arbitrary key writes that could break kernel behavior
//!    - HOW: `config:update` checks section.key against hardcoded allowlist
//!    - PROTECTED: Prevents writes to non-existent or dangerous keys
//!
//! 4. **Workspace Isolation**
//!    - WHY: Prevents cross-workspace config access
//!    - HOW: Config path resolved from syscall context working directory
//!    - PROTECTED: Agents cannot read/write config from other workspaces
//!
//! 5. **Atomic Writes**
//!    - WHY: Prevents partial config corruption on crashes
//!    - HOW: Write to temp file, then atomic rename (POSIX guarantees)
//!    - PROTECTED: Readers see old or new config, never partial writes
//!
//! CONFIGURATION SECTIONS
//! ======================
//! **[harness]** - Test framework settings
//! - `slow_idle` - Milliseconds to wait in slow idle state
//! - `deep_idle` - Milliseconds to wait in deep idle state
//!
//! **[head]** - Head agent configuration
//! - `temperature` - LLM temperature (0.0-1.0)
//! - `max_tokens` - Maximum output tokens per request
//! - `fever` - Fever mode: "baseline" | "low" | "medium" | "high"
//! - `generation` - Model generation: "sonnet" | "opus"
//! - `autist` - Autist mode (detailed/pedantic responses): bool
//! - `heartbeat_tick` - Heartbeat interval in milliseconds
//! - `debounce_ms` - Debounce delay for input processing
//! - `time_gap_marker_minutes` - Time gap threshold for conversation markers
//!
//! **[hand]** - Hand agent configuration
//! - `temperature` - LLM temperature (typically 0.0 for deterministic tool use)
//! - `max_tokens` - Maximum output tokens per request
//! - `fever` - Fever mode (affects tool call aggressiveness)
//! - `generation` - Model generation
//! - `autist` - Autist mode (verbose tool reasoning)
//! - `max_iters` - Maximum tool execution iterations per task
//! - `max_output_chars_in_prompt` - Truncation limit for tool output in prompts
//! - `max_trace_entries_in_prompt` - Maximum tool trace entries in context
//!
//! **[mind]** - Room/Mind agent configuration
//! - `temperature` - LLM temperature for reflection/planning
//! - `max_tokens` - Maximum output tokens per request
//! - `fever` - Fever mode (affects planning intensity)
//! - `generation` - Model generation
//! - `autist` - Autist mode (detailed planning output)
//! - `tact` - Tact level: "low" | "medium" | "high"
//! - `tick_interval` - Room coordinator tick interval in milliseconds
//!
//! **[tars]** - TARS personality dials (0-100 scale)
//! - `humor` - Humor level (0=serious, 100=comedian)
//! - `honesty` - Honesty level (0=diplomatic, 100=brutally honest)
//! - `sarcasm` - Sarcasm level (0=literal, 100=sarcastic)
//! - `verbosity` - Verbosity level (0=terse, 100=verbose)
//! - `confidence` - Confidence level (0=uncertain, 100=assertive)
//! - `curiosity` - Curiosity level (0=passive, 100=inquisitive)
//! - `patience` - Patience level (0=impatient, 100=patient)
//! - `formality` - Formality level (0=casual, 100=formal)
//! - `empathy` - Empathy level (0=detached, 100=empathetic)
//! - `pedantry` - Pedantry level (0=relaxed, 100=pedantic)
//! - `initiative` - Initiative level (0=reactive, 100=proactive)
//! - `optimism` - Optimism level (0=pessimistic, 100=optimistic)
//! - `caution` - Caution level (0=reckless, 100=cautious)
//!
//! PERFORMANCE
//! ===========
//! - **Read performance**: File I/O + TOML parsing (~1ms for typical 1KB config)
//! - **Write performance**: Read + modify + serialize + atomic write (~2-3ms)
//! - **No caching**: Every read hits disk (reflects manual edits immediately)
//! - **Acceptable overhead**: Config queries are infrequent (startup + occasional runtime checks)
//!
//! CONCURRENCY
//! ===========
//! - **config:read**: No locking required (read-only)
//! - **config:update**: Mutation guard serializes writes (one "head" agent at a time)
//! - **Atomic writes**: Prevent partial reads during concurrent updates
//! - **Last-write-wins**: No conflict resolution or versioning
//!
//! TRADE-OFFS
//! ==========
//! 1. **File-Based vs. Database**
//!    - CHOSEN: TOML files in `.abbot/config.toml`
//!    - REJECTED: SQLite database or in-memory storage
//!    - WHY: Human-editable, version-controllable, simpler implementation
//!    - IMPLICATION: No ACID guarantees, but acceptable for config data
//!
//! 2. **Allowlist Maintenance**
//!    - CHOSEN: Hardcoded allowlist in `config:update`
//!    - REJECTED: Dynamic allowlist from database
//!    - WHY: Compile-time validation prevents typos, forces explicit approval
//!    - IMPLICATION: Adding new config keys requires code changes + recompile
//!
//! 3. **No Caching**
//!    - CHOSEN: Always read from disk
//!    - REJECTED: In-memory cache with invalidation
//!    - WHY: Reflects manual edits immediately, simpler implementation
//!    - IMPLICATION: Slower than cached reads, but config queries are infrequent

mod read;
mod update;

pub use read::ConfigRead;
pub use update::ConfigUpdate;

use std::sync::Arc;
use crate::kernel::KernelDispatcher;

/// Register config namespace syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration for the config namespace. Called during
/// kernel initialization to register `config:read` and `config:update` handlers.
///
/// REGISTERED SYSCALLS:
/// - `config:read` - Read-only configuration queries (no mutation guard)
/// - `config:update` - Write configuration with allowlist validation (mutation guard required)
pub fn register(dispatcher: &mut KernelDispatcher) {
    dispatcher.register(Arc::new(ConfigRead::new()));
    dispatcher.register(Arc::new(ConfigUpdate::new()));
}
