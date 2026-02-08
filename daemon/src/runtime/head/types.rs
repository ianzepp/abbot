use std::collections::VecDeque;
use uuid::Uuid;

/// Context for the need currently being processed.
///
/// WHY: Encapsulates all state needed to process and resume a need, including
/// the LLM transcript, pending tool calls, and external tool signatures for
/// runaway loop detection.
///
/// INVARIANTS:
/// ----------
/// INV-1: llm_messages is a valid OpenAI-compatible transcript (alternating roles)
/// INV-2: pending_external matches the tool_call_ids registered with TurnRuntime
/// INV-3: recent_external_sigs contains at most 64 entries (bounded queue)
#[derive(Debug, Clone)]
pub(super) struct ActiveNeed {
    pub(super) need_id: String,
    pub(super) need_text: String,
    pub(super) context: String,
    pub(super) scope: Option<String>,
    pub(super) reply_to: Option<Uuid>,

    pub(super) wait_kind: Option<WaitKind>,

    /// Persisted LLM transcript for this need.
    pub(super) llm_messages: Vec<crate::hal::llm::ChatMessage>,

    /// External tool calls pending for the active segment.
    pub(super) pending_external: Vec<crate::hal::llm::ToolCall>,

    /// Recent external tool call signatures for runaway loop detection.
    pub(super) recent_external_sigs: VecDeque<u64>,
}

/// Wait reason for paused needs.
///
/// WHY: Distinguishes waiting for external tool results, which have a
/// specific resume mechanism via the TurnRuntime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WaitKind {
    ExternalTool,
}

/// Resume message sent to the head run loop.
///
/// WHY: Supports two resume paths: new need lease and external tool results.
#[derive(Debug, Clone)]
pub(super) enum ResumeMsg {
    ExternalTools {
        results: Vec<crate::kernel::ExternalToolResult>,
    },
    Need(ActiveNeed),
}
