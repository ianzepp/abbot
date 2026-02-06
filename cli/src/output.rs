//! Output - Response formatting for CLI display
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! All command modules delegate output to this module via `print_response()`.
//! Two formats are supported:
//! - `json`: One compact JSON object per line (NDJSON), suitable for piping
//!   into `jq` or other tooling.
//! - `pretty`: Human-readable hierarchical display with key: value layout.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - The json format is the default because the CLI is primarily a scripting
//!   tool. Machine-readable output is the common case.
//! - Pretty format is intentionally simple (no color, no tables). It is a
//!   quick-look aid, not a replacement for the TUI.
//! - Items are printed one per line (json) or separated by `---` (pretty),
//!   matching the streaming nature of the RPC protocol.

use serde_json::Value;

use crate::client::RpcResponse;

// =============================================================================
// FORMAT SELECTION
// =============================================================================

/// Output format for CLI responses.
///
/// WHY clap::ValueEnum: Lets clap parse --format=json|pretty directly from
/// the command line without manual string matching.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum OutputFormat {
    /// One compact JSON object per line (NDJSON). Default for scripting.
    #[default]
    Json,
    /// Human-readable hierarchical display.
    Pretty,
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
    match format {
        OutputFormat::Json => print_json(resp),
        OutputFormat::Pretty => print_pretty(resp),
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
