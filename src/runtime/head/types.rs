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
    pub(super) pending_task_ids: Vec<String>,

    /// Persisted LLM transcript for this need.
    pub(super) llm_messages: Vec<crate::llm::ChatMessage>,

    /// External tool calls pending for the active segment.
    pub(super) pending_external: Vec<crate::llm::ToolCall>,

    /// Recent external tool call signatures for runaway loop detection.
    pub(super) recent_external_sigs: VecDeque<u64>,
}

/// Wait reason for paused needs.
///
/// WHY: Distinguishes between waiting for internal proc tasks vs waiting for
/// external tool results, which have different resume mechanisms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WaitKind {
    Tasks,
    ExternalTool,
}

/// Resume message sent to the head run loop.
///
/// WHY: Supports three resume paths: new need lease, external tool results,
/// and internal task completion.
#[derive(Debug, Clone)]
pub(super) enum ResumeMsg {
    ExternalTools {
        results: Vec<crate::kernel::ExternalToolResult>,
    },
    Need(ActiveNeed),
    TasksDone {
        need_id: String,
    },
}
