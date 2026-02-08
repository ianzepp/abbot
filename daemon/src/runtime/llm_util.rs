//! LLM Utilities - Stream frame accumulation for LLM syscalls.
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! LLM syscalls stream frames containing incremental text deltas and tool calls.
//! Multiple services used to parse these streams independently, which made the
//! logic harder to test and reason about. This module centralizes the parsing
//! logic into a small accumulator that can be unit-tested with synthetic frames.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Deterministic core: Parsing is pure and stateful, without kernel access.
//! - Low ceremony: A small API mirrors the streaming loop structure.
//! - Behavior parity: Parsing mirrors existing per-service logic to avoid
//!   behavioral drift during refactors.

use serde_json::Value;

use crate::hal::llm::ToolCall;
use crate::kernel::{Frame, FrameOp};

// =============================================================================
// TYPES
// =============================================================================

/// Controls how accumulated text is converted into an optional result.
///
/// WHY: Different callers historically treated whitespace-only output
/// differently. This preserves that behavior while centralizing parsing.
pub(crate) enum LlmContentMode {
    /// Empty string becomes None.
    EmptyIsNone,
    /// Whitespace-only string becomes None.
    WhitespaceIsNone,
}

/// Accumulates streamed LLM frames into text and tool calls.
///
/// WHY this exists: Several services independently parse `llm:chat` frame
/// streams. Consolidating the parser enables focused unit tests and reduces
/// drift between loops.
pub(crate) struct LlmFrameAccumulator {
    content: String,
    tool_calls: Vec<ToolCall>,
}

impl LlmFrameAccumulator {
    /// Create an empty accumulator.
    pub(crate) fn new() -> Self {
        Self {
            content: String::new(),
            tool_calls: Vec::new(),
        }
    }

    /// Process a single streamed frame.
    ///
    /// WHY: The stream drives control flow. Returning `true` allows callers
    /// to break out on `FrameOp::Done` without duplicating match logic.
    pub(crate) fn process_frame(&mut self, frame: &Frame) -> Result<bool, String> {
        match frame.op {
            FrameOp::Item => {
                let Some(data) = frame.data.as_ref() else {
                    return Ok(false);
                };

                match data.get("type").and_then(|v| v.as_str()) {
                    Some("text_delta") => {
                        if let Some(text) = data.get("content").and_then(|v| v.as_str()) {
                            self.content.push_str(text);
                        }
                    }
                    Some("tool_call") => {
                        self.push_tool_call(data);
                    }
                    _ => {}
                }

                Ok(false)
            }
            FrameOp::Error => Err(extract_error_message(frame)),
            FrameOp::Done => Ok(true),
            _ => Ok(false),
        }
    }

    /// Convert accumulated state into text + tool calls.
    pub(crate) fn into_parts(self) -> (String, Vec<ToolCall>) {
        (self.content, self.tool_calls)
    }

    /// Convert accumulated state into the caller's preferred content mode.
    pub(crate) fn into_result(self, mode: LlmContentMode) -> (Option<String>, Vec<ToolCall>) {
        let (content, tool_calls) = self.into_parts();

        let empty = match mode {
            LlmContentMode::EmptyIsNone => content.is_empty(),
            LlmContentMode::WhitespaceIsNone => content.trim().is_empty(),
        };

        let content = if empty { None } else { Some(content) };

        (content, tool_calls)
    }

    /// Parse and store a tool call item.
    ///
    /// WHY: Tool calls arrive as untyped JSON. We mirror the existing
    /// extraction rules to avoid behavioral changes.
    fn push_tool_call(&mut self, data: &Value) {
        let id = data
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let arguments_v = data
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let arguments = serde_json::to_string(&arguments_v)
            .ok()
            .filter(|s| s.trim_start().starts_with('{'))
            .unwrap_or_else(|| "{}".to_string());

        if id.is_empty() || name.is_empty() {
            return;
        }

        let value = serde_json::json!({
            "id": id,
            "type": "function",
            "function": {"name": name, "arguments": arguments}
        });

        if let Ok(tc) = serde_json::from_value::<ToolCall>(value) {
            self.tool_calls.push(tc);
        }
    }
}

// =============================================================================
// HELPERS
// =============================================================================

/// Extract a best-effort error message from a frame.
///
/// WHY: Error frames can omit a message; we still need a stable fallback.
fn extract_error_message(frame: &Frame) -> String {
    frame
        .data
        .as_ref()
        .and_then(|d| d.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or("llm syscall error")
        .to_string()
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::{LlmContentMode, LlmFrameAccumulator};
    use crate::kernel::Frame;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn accumulates_text_deltas() {
        let mut acc = LlmFrameAccumulator::new();
        let parent = Uuid::new_v4();

        let f1 = Frame::item(parent, json!({"type": "text_delta", "content": "hello "}));
        let f2 = Frame::item(parent, json!({"type": "text_delta", "content": "world"}));

        assert!(!acc.process_frame(&f1).unwrap());
        assert!(!acc.process_frame(&f2).unwrap());

        let (content, tool_calls) = acc.into_result(LlmContentMode::EmptyIsNone);
        assert_eq!(content.as_deref(), Some("hello world"));
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn accumulates_tool_calls() {
        let mut acc = LlmFrameAccumulator::new();
        let parent = Uuid::new_v4();

        let f1 = Frame::item(
            parent,
            json!({
                "type": "tool_call",
                "tool_call_id": "call_1",
                "name": "tool__fs_read",
                "arguments": {"path": "/tmp/file.txt"}
            }),
        );

        assert!(!acc.process_frame(&f1).unwrap());

        let (content, tool_calls) = acc.into_result(LlmContentMode::EmptyIsNone);
        assert_eq!(content, None);
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "tool__fs_read");
        assert_eq!(
            tool_calls[0].function.arguments,
            "{\"path\":\"/tmp/file.txt\"}"
        );
    }

    #[test]
    fn whitespace_only_content_can_be_suppressed() {
        let mut acc = LlmFrameAccumulator::new();
        let parent = Uuid::new_v4();

        let f1 = Frame::item(parent, json!({"type": "text_delta", "content": "   "} ));
        assert!(!acc.process_frame(&f1).unwrap());

        let (content, _) = acc.into_result(LlmContentMode::WhitespaceIsNone);
        assert_eq!(content, None);
    }

    #[test]
    fn error_frames_surface_message() {
        let mut acc = LlmFrameAccumulator::new();
        let parent = Uuid::new_v4();

        let f1 = Frame::error(parent, json!({"message": "boom"}));
        let err = acc.process_frame(&f1).unwrap_err();
        assert_eq!(err, "boom");
    }
}
