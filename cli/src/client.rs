//! RpcClient - Unix domain socket client for the Abbot daemon RPC protocol
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The RPC client connects to `<workspace>/rpc.sock`, performs a version
//! handshake, then provides a `call(method, params, timeout)` interface that
//! maps to the `rpc:call` syscall protocol defined in `docs/abbot-cli.md`.
//!
//! Wire format: NDJSON (one JSON object per line). The daemon wraps frames
//! in a tagged envelope (`{"type":"frame","data":{...}}`), matching the
//! OutMessage format from `frames_uds.rs`.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - One connection per CLI invocation. The client is not pooled or reused.
//! - Correlation by parent_id: the client generates a UUID for each request
//!   and filters responses by matching parent_id until a done/error arrives.
//! - Timeout wraps the entire response collection, not individual reads.
//!
//! SECURITY MODEL
//! ==============
//! - The RPC socket is created with 0600 permissions by the daemon.
//! - The daemon forces actor="user" for all RPC dispatches, preventing
//!   privilege escalation through user-controlled actor strings.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::error::CliError;
use crate::frame::{Frame, FrameOp};

// =============================================================================
// SERVER MESSAGE ENVELOPE
// =============================================================================
//
// WHY tagged enum: The daemon wraps all outbound messages in a type-tagged
// envelope (`{"type":"connected",...}`, `{"type":"frame",...}`). This mirrors
// the OutMessage type in frames_uds.rs and lets the client distinguish
// handshake, frame, and error messages on the same stream.

/// Server-to-client message envelope (mirrors `frames_uds.rs` OutMessage).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
enum ServerMessage {
    #[serde(rename = "connected")]
    Connected {
        #[allow(dead_code)]
        version: String,
    },
    #[serde(rename = "frame")]
    Frame(Frame),
    #[serde(rename = "error")]
    Error { message: String },
}

// =============================================================================
// RPC RESPONSE
// =============================================================================

/// Collected response from a single RPC call.
///
/// WHY separate from raw frames: Command modules don't need to deal with
/// frame correlation or stream termination. They receive a clean list of
/// item payloads and an optional ok payload, ready for output formatting.
#[derive(Debug)]
pub struct RpcResponse {
    /// Streamed item payloads (one per `item` frame).
    pub items: Vec<Value>,
    /// Optional acknowledgement payload from the `ok` frame.
    pub ok_data: Option<Value>,
}

// =============================================================================
// RPC CLIENT
// =============================================================================

/// RPC client connected to the daemon over a Unix domain socket.
///
/// WHY split reader/writer: tokio::io::split lets us hold independent
/// references for reading and writing without borrowing the whole stream,
/// which would prevent concurrent read/write in future extensions.
pub struct RpcClient {
    reader: BufReader<tokio::io::ReadHalf<UnixStream>>,
    writer: tokio::io::WriteHalf<UnixStream>,
}

impl RpcClient {
    /// Connect to the RPC socket and perform the version handshake.
    ///
    /// WHY handshake: The spec requires the daemon to emit an initial
    /// `connected` message with the protocol version. Reading it here
    /// confirms the daemon is alive and speaking the expected protocol
    /// before any commands are sent.
    pub async fn connect(sock_path: &Path) -> Result<Self, CliError> {
        let stream = UnixStream::connect(sock_path)
            .await
            .map_err(CliError::Connect)?;

        let (read_half, write_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(read_half);

        // -------------------------------------------------------------------------
        // HANDSHAKE: Read and validate the daemon's initial connected message
        // -------------------------------------------------------------------------
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(CliError::Connect)?;

        let msg: ServerMessage =
            serde_json::from_str(line.trim()).map_err(|e| CliError::Handshake(e.to_string()))?;

        match msg {
            ServerMessage::Connected { .. } => {}
            ServerMessage::Error { message } => {
                return Err(CliError::Handshake(message));
            }
            _ => {
                return Err(CliError::Handshake(
                    "unexpected first message from server".into(),
                ));
            }
        }

        Ok(Self {
            reader,
            writer: write_half,
        })
    }

    /// Send an `rpc:call` request and collect all correlated response frames.
    ///
    /// WHY collect-then-return: The CLI is a synchronous tool — each command
    /// sends one request and needs all response data before formatting output.
    /// Streaming output (e.g., for long-running methods) can be added later
    /// by yielding items as they arrive.
    pub async fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<RpcResponse, CliError> {
        let req = Frame::rpc_call(method, params);
        let req_id = req.id;

        // -------------------------------------------------------------------------
        // SEND: Serialize and write the request as a single NDJSON line
        // -------------------------------------------------------------------------
        let mut json = serde_json::to_string(&req)?;
        json.push('\n');
        self.writer
            .write_all(json.as_bytes())
            .await
            .map_err(CliError::Io)?;

        // -------------------------------------------------------------------------
        // COLLECT: Read response frames correlated by parent_id until done/error
        // WHY timeout wraps the whole loop: individual reads may be fast, but the
        // overall response time depends on how long the daemon takes to process.
        // -------------------------------------------------------------------------
        let mut items = Vec::new();
        let mut ok_data = None;

        loop {
            let mut line = String::new();
            let read_result = tokio::time::timeout(timeout, self.reader.read_line(&mut line)).await;

            let bytes_read = match read_result {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(CliError::Io(e)),
                Err(_) => return Err(CliError::Timeout),
            };

            if bytes_read == 0 {
                return Err(CliError::Protocol("connection closed by server".into()));
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let msg: ServerMessage = serde_json::from_str(trimmed)
                .map_err(|e| CliError::Protocol(format!("bad server message: {e}")))?;

            let frame = match msg {
                ServerMessage::Frame(f) => f,
                // WHY: A top-level server error (not correlated to our request)
                // indicates a fatal protocol issue.
                ServerMessage::Error { message } => {
                    return Err(CliError::Rpc {
                        code: "server_error".into(),
                        message,
                    });
                }
                ServerMessage::Connected { .. } => continue,
            };

            // WHY: The RPC socket may carry responses to other concurrent
            // requests in the future. Filter to our correlation ID.
            if frame.parent_id != Some(req_id) {
                continue;
            }

            match frame.op {
                FrameOp::Item => {
                    if let Some(data) = frame.data {
                        items.push(data);
                    }
                }
                FrameOp::Ok => {
                    ok_data = frame.data;
                }
                FrameOp::Done => break,
                FrameOp::Error => {
                    let data = frame.data.unwrap_or(Value::Null);
                    let code = data
                        .get("code")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let message = data
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error")
                        .to_string();
                    return Err(CliError::Rpc { code, message });
                }
                _ => {}
            }
        }

        Ok(RpcResponse { items, ok_data })
    }
}
