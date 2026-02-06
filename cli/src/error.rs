//! CliError - Unified error type for the CLI client
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Every failure mode the CLI can encounter maps to a CliError variant. The
//! variants follow the request lifecycle: config resolution -> socket connect
//! -> handshake -> send/receive -> parse response. This ordering makes errors
//! self-documenting when displayed to the user.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - User-facing messages are lowercase, terse, and actionable.
//! - The Display impl produces the full error string; main.rs just prints it.
//! - From impls for std::io::Error and serde_json::Error allow `?` propagation
//!   throughout the client code without manual mapping at every call site.

use std::fmt;

// =============================================================================
// ERROR TYPE
// =============================================================================

/// Unified error type covering all CLI failure modes.
///
/// WHY unified: A single error type simplifies the `run()` return path and
/// ensures every error reaches the user with consistent formatting. Variants
/// are ordered by the phase in which they occur.
#[derive(Debug)]
pub enum CliError {
    /// Could not find or parse config (workspace resolution failed).
    Config(String),
    /// Socket connection failed (daemon not running, wrong path).
    Connect(std::io::Error),
    /// Protocol handshake failed (incompatible version, unexpected message).
    Handshake(String),
    /// Serialization/deserialization error (malformed NDJSON on the wire).
    Protocol(String),
    /// The daemon returned an error frame with a structured code + message.
    Rpc { code: String, message: String },
    /// Request timed out waiting for a done/error frame.
    Timeout,
    /// I/O error during mid-stream communication.
    Io(std::io::Error),
}

// =============================================================================
// DISPLAY
// =============================================================================

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "config error: {msg}"),
            Self::Connect(e) => write!(f, "connection failed: {e}"),
            Self::Handshake(msg) => write!(f, "handshake failed: {msg}"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Self::Rpc { code, message } => write!(f, "rpc error [{code}]: {message}"),
            Self::Timeout => write!(f, "request timed out"),
            Self::Io(e) => write!(f, "i/o error: {e}"),
        }
    }
}

impl std::error::Error for CliError {}

// =============================================================================
// CONVERSIONS
// =============================================================================
//
// WHY From impls: The client module performs many I/O and serde operations.
// These conversions let us use `?` throughout without explicit map_err on
// every call, keeping the happy path readable.

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        Self::Protocol(e.to_string())
    }
}
