//! CliError - Unified error type for the CLI client

use std::fmt;

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
    /// General error from moved commands that use Box<dyn Error>.
    General(String),
}

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
            Self::General(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CliError {}

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

impl From<Box<dyn std::error::Error>> for CliError {
    fn from(e: Box<dyn std::error::Error>) -> Self {
        Self::General(e.to_string())
    }
}

impl From<String> for CliError {
    fn from(s: String) -> Self {
        Self::General(s)
    }
}

impl From<&str> for CliError {
    fn from(s: &str) -> Self {
        Self::General(s.to_string())
    }
}
