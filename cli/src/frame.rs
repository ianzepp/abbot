//! Frame - Local wire protocol types for CLI-to-daemon communication
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module provides a standalone copy of the kernel's Frame and FrameOp types,
//! scoped to what the CLI client needs. The CLI is a separate workspace crate with
//! no dependency on the main abbot crate, so we replicate the serde-compatible
//! structure here rather than pulling in the full kernel dependency graph.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Serde attributes MUST match the kernel's frame.rs exactly, or NDJSON
//!   round-trips between client and daemon will silently fail.
//! - Only constructors needed by the CLI are provided (rpc_call, cancel).
//!   Response frames are deserialized, not constructed.
//!
//! TRADE-OFF: Duplicating Frame types means changes to the kernel's frame.rs
//! require a manual sync here. This is acceptable because the wire protocol
//! should be stable, and it avoids coupling the CLI binary to the full runtime.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// =============================================================================
// FRAME OPERATIONS
// =============================================================================
//
// Mirrors kernel::frame::FrameOp. The rename_all = "snake_case" attribute is
// critical: the daemon serializes ops as lowercase strings on the wire.

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameOp {
    Req,
    Cancel,
    Ok,
    Error,
    Done,
    Item,
    Bytes,
    Event,
    Progress,
}

// =============================================================================
// FRAME STRUCTURE
// =============================================================================
//
// Mirrors kernel::frame::Frame. All optional fields use skip_serializing_if
// to minimize wire overhead, matching the kernel's NDJSON output exactly.

/// Core frame for syscall request/response communication over the RPC socket.
///
/// WHY local copy: The CLI crate is intentionally decoupled from the main abbot
/// crate to keep the binary small and avoid pulling in tokio runtime, SQLite,
/// and other daemon-only dependencies. The wire format is the contract.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    /// Unique frame identifier for request correlation.
    pub id: Uuid,
    /// Creation timestamp (milliseconds since Unix epoch).
    #[serde(default)]
    pub ts: i64,
    /// Distinguishes request vs response vs stream emission.
    pub op: FrameOp,
    /// Syscall name (<namespace>:<verb>) — required for Req, optional otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Links response frames back to the originating request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    /// Authorship identity (e.g. "user"). Daemon forces actor="user" for RPC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Timeout enforcement for syscall execution (milliseconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    /// Observability metadata (scope, span, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<Value>,
    /// Operation-specific payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// =============================================================================
// FRAME CONSTRUCTORS
// =============================================================================
//
// Only two constructors are needed for the CLI: rpc_call (the single RPC
// entrypoint per the spec) and cancel (for graceful request cancellation).

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl Frame {
    /// Build an `rpc:call` request frame.
    ///
    /// WHY single entrypoint: The RPC spec routes all CLI operations through
    /// one syscall name (`rpc:call`), with the logical method name inside
    /// `data.method`. This keeps the wire protocol simple and the daemon's
    /// dispatch centralized.
    pub fn rpc_call(method: &str, params: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            ts: now_ms(),
            op: FrameOp::Req,
            name: Some("rpc:call".into()),
            parent_id: None,
            actor: None,
            deadline_ms: None,
            trace: None,
            data: Some(serde_json::json!({
                "method": method,
                "params": params,
                "stream": true,
            })),
        }
    }

    /// Build a cancel frame targeting a previous request.
    ///
    /// WHY: The spec allows clients to cancel in-flight requests by sending
    /// a cancel frame with parent_id pointing at the original request. Used
    /// on timeout or user interrupt before disconnecting.
    #[allow(dead_code)]
    pub fn cancel(target_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            ts: now_ms(),
            op: FrameOp::Cancel,
            name: None,
            parent_id: Some(target_id),
            actor: None,
            deadline_ms: None,
            trace: None,
            data: None,
        }
    }
}
