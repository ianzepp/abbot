//! Text:Echo - Return input text unchanged
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! A minimal identity operation that returns its input text without modification.
//! Useful for testing tool calling infrastructure, validating argument parsing,
//! and debugging LLM integrations without triggering side effects.
//!
//! **Integration:** Called by "hand" agents via the `tool__text_echo` tool spec.
//! Frequently used in development/testing scenarios to verify tool dispatch works
//! correctly before attempting more complex operations.
//!
//! **Frame protocol:**
//! - Emits `Frame::ok` with `{"text": <input>}`
//! - Returns `E_INVALID_ARGS` if text field is missing or malformed
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Zero side effects**: Truly read-only operation, safe to call repeatedly
//! - **Testing utility**: Enables verification of tool calling without I/O
//! - **Simple debugging**: Echo back args to validate JSON deserialization
//! - **Performance baseline**: Minimal syscall overhead for benchmarking
//!
//! USE CASES
//! =========
//! - **LLM integration testing**: Verify tool calls reach syscall layer correctly
//! - **Argument validation**: Confirm JSON parsing works before complex operations
//! - **Latency baseline**: Measure syscall dispatch overhead without I/O

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};

// =============================================================================
// ARGUMENTS
// =============================================================================

/// Arguments for `text:echo` syscall.
///
/// WHY: Single required field keeps the interface minimal. No defaults needed
/// since the operation is meaningless without input text.
#[derive(Debug, Deserialize)]
struct TextEchoArgs {
    /// Text to echo back unchanged.
    text: String,
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for returning input text unchanged.
///
/// WHY: Stateless unit struct since echo requires no configuration or state.
pub struct TextEcho;

impl TextEcho {
    /// Create a new `TextEcho` syscall.
    ///
    /// WHY: Standard constructor for consistency with other syscalls.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TextEcho {
    fn name(&self) -> &'static str {
        "text:echo"
    }

    /// Echo input text back to the caller.
    ///
    /// WHY: Provides a zero-side-effect operation for testing tool calling
    /// infrastructure. Useful for validating that LLM tool calls reach the
    /// syscall layer correctly and that JSON argument parsing works.
    ///
    /// USE CASE: Invoked during development to test tool dispatch without
    /// triggering filesystem writes, network requests, or other side effects.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"text": <input>}`
    /// - `E_INVALID_ARGS` if text field is missing or JSON is malformed
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // WHY: Check cancellation before deserialization to fail fast if caller
        // cancelled during queueing
        ctx.check_cancelled()?;

        let args: TextEchoArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        // WHY: Echo the text back unchanged. Ignoring send error is safe since
        // the caller may have already disconnected, and we don't need backpressure
        // for this simple operation
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"text": args.text})))
            .await;

        Ok(())
    }
}
