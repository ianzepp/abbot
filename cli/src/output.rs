//! Output - Response formatting for CLI display
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! All command modules delegate output to this module via `print_response()`
//! (for RPC commands) or `print_value()` (for offline commands).
//!
//! Three formats are supported:
//! - `auto` (default): Detects whether stdout is a TTY. TTY → Pretty, pipe → Json.
//! - `json`: One compact JSON object per line (NDJSON), suitable for piping
//!   into `jq` or other tooling.
//! - `pretty`: Human-readable hierarchical display with key: value layout.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Auto mode means humans get readable output and scripts get JSON with no
//!   flags needed. This enables a future `cli:command` daemon syscall where
//!   the agent calls `abbot providers list` and gets machine-readable JSON
//!   automatically.
//! - Pretty format is intentionally simple (no color, no tables). It is a
//!   quick-look aid, not a replacement for the TUI.
//! - Items are printed one per line (json) or separated by `---` (pretty),
//!   matching the streaming nature of the RPC protocol.

use std::io::IsTerminal;

use serde_json::Value;

use crate::client::RpcResponse;

// =============================================================================
// FORMAT SELECTION
// =============================================================================

/// Output format for CLI responses.
///
/// WHY clap::ValueEnum: Lets clap parse --format=json|pretty|auto directly
/// from the command line without manual string matching.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum OutputFormat {
    /// Auto-detect: pretty for TTY, JSON for pipes. Default.
    #[default]
    Auto,
    /// One compact JSON object per line (NDJSON).
    Json,
    /// Human-readable hierarchical display.
    Pretty,
}

impl OutputFormat {
    /// Resolve `Auto` into a concrete format based on whether stdout is a TTY.
    pub fn resolve(self) -> Self {
        match self {
            Self::Auto => {
                if std::io::stdout().is_terminal() {
                    Self::Pretty
                } else {
                    Self::Json
                }
            }
            other => other,
        }
    }
}

// =============================================================================
// CORE
// =============================================================================

/// Format and print an RPC response to stdout.
///
/// WHY centralized: Every command module has the same output need. Routing
/// through one function ensures consistent behavior across all commands and
/// makes it easy to add new formats later.
pub fn print_response(resp: &RpcResponse, format: OutputFormat) {
    match format.resolve() {
        OutputFormat::Json => print_json(resp),
        OutputFormat::Pretty => print_pretty(resp),
        OutputFormat::Auto => unreachable!(),
    }
}

/// Format and print a `serde_json::Value` to stdout.
///
/// Entry point for offline commands that build structured data directly
/// rather than going through the RPC client.
pub fn print_value(val: &Value, format: OutputFormat) {
    match format.resolve() {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string(val).unwrap_or_default());
        }
        OutputFormat::Pretty => {
            print_value_pretty(val, 0);
        }
        OutputFormat::Auto => unreachable!(),
    }
}

// =============================================================================
// FORMATTERS
// =============================================================================

/// Print response as NDJSON (one JSON object per line).
///
/// WHY items-first: If the response contains streamed items, those are the
/// primary data. The ok payload is only printed when there are no items
/// (e.g., status.info returns a single ok with all data).
fn print_json(resp: &RpcResponse) {
    if resp.items.is_empty() {
        if let Some(ref ok) = resp.ok_data {
            println!("{}", serde_json::to_string(ok).unwrap_or_default());
        }
    } else {
        for item in &resp.items {
            println!("{}", serde_json::to_string(item).unwrap_or_default());
        }
    }
}

/// Print response in human-readable format.
fn print_pretty(resp: &RpcResponse) {
    if resp.items.is_empty() {
        if let Some(ref ok) = resp.ok_data {
            println!("{}", serde_json::to_string_pretty(ok).unwrap_or_default());
        } else {
            println!("(no data)");
        }
    } else {
        for (i, item) in resp.items.iter().enumerate() {
            if i > 0 {
                println!("---");
            }
            print_value_pretty(item, 0);
        }
    }
}

// =============================================================================
// HELPERS
// =============================================================================

/// Recursively print a JSON value with indentation.
///
/// WHY recursive: RPC responses can contain nested objects (e.g., agent status
/// with sub-fields). Flat key:value would lose structure; full JSON pretty-print
/// is too verbose. This strikes a middle ground.
fn print_value_pretty(val: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    match val {
        Value::Object(map) => {
            for (k, v) in map {
                match v {
                    Value::Object(_) | Value::Array(_) => {
                        println!("{pad}{k}:");
                        print_value_pretty(v, indent + 2);
                    }
                    _ => {
                        println!("{pad}{k}: {}", format_scalar(v));
                    }
                }
            }
        }
        Value::Array(arr) => {
            for v in arr {
                print_value_pretty(v, indent);
            }
        }
        other => {
            println!("{pad}{}", format_scalar(other));
        }
    }
}

/// Format a scalar JSON value for display.
fn format_scalar(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Null => "(null)".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}
