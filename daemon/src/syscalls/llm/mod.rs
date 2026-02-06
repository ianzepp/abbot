//! LLM Syscalls - Language model invocation and chaos engineering
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This namespace provides controlled access to Large Language Model (LLM) providers
//! within the Abbot kernel's security model. It implements three primary syscalls:
//!
//! - `llm:chat` - Invoke LLMs with messages and tools, with retry logic and streaming
//! - `llm:chaos` - Generate chaos traits for behavioral variation in agents
//! - `llm:chaos:list` - List available chaos axes and trait levels
//!
//! **Integration points:**
//! - `OpenAICompatClient` for HTTP-based LLM API communication
//! - `llm_harness::chat_with_tools_retry_on_model` for retry logic and tool handling
//! - `Store` for persisting LLM request/response history
//! - Actor-specific LLM configurations (HeadConfig, HandConfig, RoomConfig)
//!
//! **Frame protocol:**
//! - `Frame::event` with `kind: "llm:begin"` - Signals LLM invocation start
//! - `Frame::event` with `kind: "llm:retry"` - Signals retry attempt with note
//! - `Frame::item` with `type: "thinking"` - Extracted `<thinking>` tag content
//! - `Frame::item` with `type: "text_delta"` - Visible content (non-thinking text)
//! - `Frame::item` with `type: "tool_call"` - LLM-generated tool invocation
//! - `Frame::event` with `kind: "llm:result"` - Final result with usage/JSON payloads
//! - `Frame::done` - Signals completion of LLM operation
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Actor-specific configuration**: Each agent type (head/hand/mind) has independent LLM settings
//! - **Retry resilience**: Exponential backoff with model fallback for API failures
//! - **Thinking transparency**: Parse and separate `<thinking>` tags from visible output
//! - **Tool-calling support**: Pass tool specifications to LLM, receive structured tool calls
//! - **Request/response persistence**: Store full LLM interactions for debugging and analysis
//! - **Chaos engineering**: Inject behavioral variation for testing agent robustness
//!
//! STREAMING PROTOCOL
//! ==================
//! WHY Frame-based streaming: LLM responses are processed asynchronously with multiple
//! event types (thinking, text, tool calls). Frame protocol enables:
//! 1. Progress updates during retries (`llm:retry` events)
//! 2. Incremental content delivery (thinking + text deltas)
//! 3. Structured tool call emission (parsed from LLM response)
//! 4. Final metadata (usage stats, raw JSON for debugging)
//!
//! SEQUENCE:
//! ```text
//! llm:begin → [llm:retry]* → [thinking]? → [text_delta]? → [tool_call]* → llm:result → done
//! ```
//!
//! ACTOR CONFIGURATION
//! ===================
//! LLM syscalls use actor prefixes to determine configuration:
//! - `head/*` - HeadConfig (decision-making agent, typically GPT-4 or Claude)
//! - `hand/*` - HandConfig (tool execution agent, typically GPT-3.5 or Claude Haiku)
//! - `mind/*` - RoomConfig (reflection/room agents, typically Claude Opus)
//!
//! WHY: Different agent types have different requirements (speed vs. quality).
//! Head agents need powerful models for reasoning, hand agents need fast models
//! for quick tool execution, room agents need long-context models for reflection.
//!
//! RETRY LOGIC
//! ===========
//! The `llm:chat` syscall uses `RetryPolicy::default_llm()` with:
//! - Exponential backoff starting at 2s, max 32s
//! - Up to 5 retry attempts
//! - Model fallback on repeated failures (e.g., GPT-4 → GPT-3.5)
//! - Retry on 5xx errors, rate limits (429), timeouts
//! - NO retry on 4xx client errors (invalid request, auth failure)
//!
//! WHY: LLM APIs are unreliable (rate limits, transient 5xx errors). Retry logic
//! with model fallback ensures agents can make progress even when primary model
//! is unavailable. Client errors indicate bugs in request formatting, not transient issues.
//!
//! CANCELLATION
//! ============
//! LLM requests respect `SyscallContext::cancel` token:
//! - Passed to `chat_with_tools_retry_on_model` as `Option<CancellationToken>`
//! - HTTP requests abort on cancellation (HAL layer enforces)
//! - Retry loop terminates immediately on cancellation
//!
//! WHY: Long-running LLM requests (especially with retries) must not block task
//! lanes indefinitely. Cancellation enables graceful termination when parent task
//! is cancelled or times out.
//!
//! REGISTERED SYSCALLS
//! ===================
//! - `llm:chat` - Invoke LLM with messages/tools (complex, multi-phase)
//! - `llm:chaos` - Generate chaos trait prompt (simple utility)
//! - `llm:chaos:list` - List available chaos axes (simple query)

mod chat;
mod chaos;
mod chaos_list;

pub use chat::LlmChat;
pub use chaos::LlmChaos;
pub use chaos_list::LlmChaosList;

use regex::Regex;

use crate::kernel::KernelError;
use crate::runtime::{HandConfig, HeadConfig, RoomConfig};

// =============================================================================
// ACTOR CONFIGURATION HELPERS
// =============================================================================
//
// WHY: Actor-specific LLM configuration enables different agent types to use
// different models (head uses GPT-4, hand uses GPT-3.5, etc.). Configuration
// is loaded from environment or config files based on actor prefix.

/// Load LLM configuration for a given actor.
///
/// WHY: Each agent type (head/hand/mind) has independent LLM settings (model,
/// temperature, max_tokens). This function maps actor prefix to configuration.
///
/// SECURITY NOTE: Returns error if actor lacks valid prefix or LLM is disabled
/// for that actor type. Prevents unauthorized LLM usage.
pub(crate) fn cfg_for_actor(actor: &str) -> Result<crate::runtime::Config, KernelError> {
    let a = actor.trim();
    let cfg = if a.starts_with("head/") {
        HeadConfig::from_config().llm
    } else if a.starts_with("hand/") {
        HandConfig::from_config().llm
    } else if a.starts_with("mind/") {
        RoomConfig::from_config().llm
    } else {
        return Err(KernelError::invalid_args(
            "llm:chat requires actor prefix head/*, hand/*, or mind/*",
        ));
    };

    if !cfg.enabled {
        return Err(KernelError::invalid_args(format!(
            "LLM not configured for actor '{actor}'",
        )));
    }
    Ok(cfg)
}

// =============================================================================
// CONTENT PARSING HELPERS
// =============================================================================
//
// WHY: LLM responses often contain `<thinking>` tags for internal reasoning
// (inspired by Claude's extended thinking mode). Separating thinking from
// visible content enables:
// 1. Debugging agent reasoning without exposing it to users
// 2. Cleaner output in chat interfaces
// 3. Structured analysis of model's reasoning process

/// Parse LLM content, extracting `<thinking>` tags from visible text.
///
/// WHY: Enables models to include private reasoning steps without polluting
/// visible output. Supports multiple `<thinking>` blocks, joined with newlines.
///
/// RETURNS: `(thinking: Option<String>, visible: Option<String>)`
/// - `thinking` - Concatenated content of all `<thinking>` tags, or None
/// - `visible` - Remaining content after removing `<thinking>` tags, or None
///
/// EXAMPLE:
/// ```text
/// Input: "Hello <thinking>hmm...</thinking> World"
/// Output: (Some("hmm..."), Some("Hello  World"))
/// ```
pub(crate) fn parse_llm_content(content: &str) -> (Option<String>, Option<String>) {
    // WHY: Regex with `[\s\S]*?` for non-greedy matching across newlines.
    // Standard `.` doesn't match newlines, so `[\s\S]` (whitespace or non-whitespace)
    // matches everything including newlines.
    let thinking_re = Regex::new(r"<thinking>([\s\S]*?)</thinking>").unwrap();

    let mut thinking_parts = Vec::new();
    let mut visible_content = content.to_string();

    // WHY: Iterate captures to extract all `<thinking>` blocks, then remove
    // them from visible content. Multiple blocks are concatenated.
    for cap in thinking_re.captures_iter(content) {
        thinking_parts.push(cap[1].to_string());
        visible_content = visible_content.replace(&cap[0], "");
    }

    let thinking = if thinking_parts.is_empty() {
        None
    } else {
        Some(thinking_parts.join("\n"))
    };

    // WHY: Trim visible content and return None if empty. Prevents emitting
    // whitespace-only frames when LLM only produces thinking content.
    let visible = visible_content.trim();
    let visible = if visible.is_empty() {
        None
    } else {
        Some(visible.to_string())
    };

    (thinking, visible)
}

// =============================================================================
// REGISTRATION
// =============================================================================

/// Register all LLM syscalls with the kernel dispatcher.
///
/// WHY: Centralizes syscall registration for the LLM namespace. Called during
/// kernel initialization to make llm:chat, llm:chaos, and llm:chaos:list available.
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(LlmChat::new()));
    dispatcher.register(Arc::new(LlmChaos::new()));
    dispatcher.register(Arc::new(LlmChaosList::new()));
}
