//! LLM:Chat - Invoke language models with retry logic and tool calling
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall provides the primary interface for invoking Large Language Models
//! within the Abbot kernel. It implements a robust, production-ready LLM invocation
//! pipeline with:
//!
//! - **Actor-based configuration**: Different agents (head/hand/mind) use different models
//! - **Retry logic with exponential backoff**: Handles transient API failures (5xx, rate limits)
//! - **Model fallback**: Automatically downgrades to cheaper/faster models on repeated failures
//! - **Tool calling support**: Passes tool specs to LLM, receives structured tool invocations
//! - **Streaming frames**: Emits thinking, text deltas, and tool calls incrementally
//! - **Request/response persistence**: Stores full LLM interactions in kernel Store
//! - **Cancellation support**: Respects parent context cancellation during retries/requests
//!
//! **Integration points:**
//! - `OpenAICompatClient` for HTTP-based LLM API communication (OpenAI, Anthropic, etc.)
//! - `llm_harness::chat_with_tools_retry_on_model` for retry orchestration
//! - `Store` for persisting LLM request/response JSON and usage stats
//! - `cfg_for_actor()` for loading actor-specific LLM configuration
//! - `parse_llm_content()` for extracting `<thinking>` tags from responses
//!
//! **Frame protocol:**
//! 1. `Frame::event(kind: "llm:begin")` - Signals LLM invocation start
//! 2. `Frame::event(kind: "llm:retry")` - Emitted on retry attempts (with attempt # and note)
//! 3. `Frame::item(type: "thinking")` - Extracted `<thinking>` tag content
//! 4. `Frame::item(type: "text_delta")` - Visible content (after removing thinking tags)
//! 5. `Frame::item(type: "tool_call")` - Each tool call from LLM response
//! 6. `Frame::event(kind: "llm:result")` - Final result with usage/request_json/response_json
//! 7. `Frame::done` - Signals completion
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Reliability over speed**: Retry logic ensures agents can make progress even when LLM APIs are flaky
//! - **Transparency**: Expose thinking, retries, and raw JSON for debugging
//! - **Model flexibility**: Support any OpenAI-compatible API (OpenAI, Anthropic, Groq, etc.)
//! - **Tool-calling first-class**: Tool specs passed directly to LLM, no post-processing
//! - **Cancellation hygiene**: Long-running requests must respect parent task cancellation
//!
//! RETRY LOGIC
//! ===========
//! WHY: LLM APIs are unreliable - rate limits (429), transient 5xx errors, network timeouts
//! are common. Without retries, agents would fail frequently and unpredictably.
//!
//! STRATEGY:
//! - **Exponential backoff**: 2s, 4s, 8s, 16s, 32s (capped at 32s)
//! - **Up to 5 attempts**: Balance between resilience and wasted time
//! - **Model fallback**: After 2 failures on primary model, try fallback model
//!   (e.g., GPT-4 → GPT-3.5, Claude Opus → Claude Sonnet)
//! - **Retry on**: 5xx errors, 429 rate limits, network timeouts
//! - **NO retry on**: 4xx client errors (invalid request, auth failure, bad JSON)
//!
//! WHY NO RETRY ON 4xx: Client errors indicate bugs in request formatting (invalid
//! JSON schema, missing API key, etc.). Retrying won't fix these - they require
//! code changes or config fixes.
//!
//! TOOL CALLING
//! ============
//! WHY: Modern LLMs (GPT-4, Claude) support structured tool calling via function
//! definitions in the API request. This enables agents to:
//! 1. Decide which tools to invoke based on user query
//! 2. Generate structured arguments (parsed from LLM JSON)
//! 3. Chain multiple tool calls in a single turn
//!
//! FORMAT: Tool specs use OpenAI function calling format:
//! ```json
//! {
//!   "type": "function",
//!   "function": {
//!     "name": "fs:read",
//!     "description": "Read a file from the filesystem",
//!     "parameters": {
//!       "type": "object",
//!       "properties": {
//!         "path": {"type": "string", "description": "File path"}
//!       },
//!       "required": ["path"]
//!     }
//!   }
//! }
//! ```
//!
//! RESPONSE: LLM returns tool calls with:
//! ```json
//! {
//!   "id": "call_abc123",
//!   "function": {"name": "fs:read", "arguments": "{\"path\": \"/etc/hosts\"}"}
//! }
//! ```
//!
//! MESSAGE FORMATTING
//! ==================
//! WHY: Different LLM providers use slightly different message formats. We use
//! OpenAI format as the baseline (role: system/user/assistant/tool) and the
//! OpenAICompatClient adapter layer handles provider-specific translation.
//!
//! ANTHROPIC DIFFERENCES:
//! - No `system` role - system message passed separately
//! - Tool results use `tool_use_id` instead of `tool_call_id`
//! - Requires `anthropic-version` header
//!
//! OPENAI FORMAT (baseline):
//! - Messages: `[{role: "system", content: "..."}, {role: "user", content: "..."}]`
//! - Tool calls: `{role: "assistant", tool_calls: [{id, function: {name, arguments}}]}`
//! - Tool results: `{role: "tool", tool_call_id: "...", content: "..."}`
//!
//! TOKEN LIMITS
//! ============
//! WHY: LLM APIs enforce max_tokens limits to prevent runaway generation costs.
//! Each actor's config specifies `max_tokens` (e.g., 4096 for GPT-4, 8192 for Claude).
//!
//! ENFORCEMENT: Passed to `OpenAICompatClient` constructor, then included in
//! every API request. LLM stops generating when limit reached.
//!
//! TRADE-OFF: Very long responses may be truncated. Agents must handle incomplete
//! responses gracefully (e.g., by checking if response ends mid-sentence and
//! retrying with "continue" prompt).
//!
//! CANCELLATION
//! ============
//! WHY: LLM requests can take 10-30 seconds, especially with retries. If parent
//! task is cancelled (user abort, timeout), we must terminate the request to
//! avoid wasted API calls and blocking task lanes.
//!
//! HOW: Pass `ctx.cancel` token to `chat_with_tools_retry_on_model`, which:
//! 1. Checks cancellation before each retry attempt
//! 2. Passes token to HTTP client for request-level cancellation
//! 3. Returns immediately on cancellation (no partial results)
//!
//! CONCURRENCY
//! ===========
//! - No mutation lock required (LLM is read-only, no local state change)
//! - Safe for concurrent execution across multiple task lanes
//! - HTTP client uses connection pooling for efficiency
//! - Retry callback spawns detached task to avoid blocking retry loop
//!
//! PERFORMANCE
//! ===========
//! - **Latency**: 2-10s for typical requests, 10-30s with retries
//! - **Throughput**: Limited by LLM API rate limits (e.g., 60 req/min for GPT-4)
//! - **Memory**: Response buffered in memory (typically <100KB per request)
//! - **Cost**: GPT-4: $0.03/1K tokens input, $0.06/1K tokens output
//!
//! TRADE-OFFS
//! ==========
//! 1. **Retry vs. Fail Fast**
//!    - CHOSEN: Retry with exponential backoff and model fallback
//!    - REJECTED: Fail immediately on first error
//!    - WHY: LLM APIs are flaky, agents need resilience
//!    - IMPLICATION: Slower failure on persistent errors (30s vs. 2s)
//!
//! 2. **Buffered vs. Streaming**
//!    - CHOSEN: Buffer full response, then emit frames
//!    - REJECTED: Stream chunks incrementally as they arrive
//!    - WHY: Simpler implementation, easier to parse tool calls
//!    - IMPLICATION: No "typing animation" effect, users see full response at once
//!
//! 3. **OpenAI Format vs. Native Formats**
//!    - CHOSEN: OpenAI format as baseline, adapter layer for others
//!    - REJECTED: Native format for each provider
//!    - WHY: Consistency across providers, easier testing
//!    - IMPLICATION: Adapter complexity for Anthropic, Groq, etc.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::llm::{ChatMessage, ChatToolResult, OpenAICompatClient, ToolSpec};
use crate::runtime::Kernel;
use crate::runtime::llm_harness::{RetryPolicy, chat_with_tools_retry_on_model};

use super::{cfg_for_actor, parse_llm_content};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for invoking LLMs with messages and tool calling.
///
/// WHY: Encapsulates LLM invocation pipeline (retry logic, tool calling, response
/// parsing) behind a syscall interface. Enables consistent LLM access across
/// all agent types (head/hand/mind).
pub struct LlmChat;

impl LlmChat {
    /// Create a new `LlmChat` syscall.
    ///
    /// WHY: Standard constructor for syscall registration.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LlmChat {
    fn name(&self) -> &'static str {
        "llm:chat"
    }

    /// Invoke an LLM with messages and optional tool specifications.
    ///
    /// WHY: Primary interface for agent-to-LLM communication. Handles configuration
    /// loading, retry logic, tool calling, response parsing, and frame emission.
    ///
    /// USE CASE: Invoked by all agent types (head/hand/mind) to:
    /// - Generate natural language responses to user queries
    /// - Decide which tools to invoke based on context
    /// - Reflect on task progress and plan next steps
    /// - Generate code, documentation, or structured data
    ///
    /// ARGUMENTS:
    /// - `messages` (required): Array of `ChatMessage` objects with role/content
    /// - `tools` (optional): Array of `ToolSpec` objects for function calling
    /// - `tool_choice` (optional): Control tool selection ("auto", "required", specific tool)
    ///
    /// RETURNS:
    /// - Emits `Frame::event(llm:begin)` with provider/model/message count
    /// - Emits `Frame::event(llm:retry)` for each retry attempt
    /// - Emits `Frame::item(thinking)` if `<thinking>` tags found in response
    /// - Emits `Frame::item(text_delta)` for visible content
    /// - Emits `Frame::item(tool_call)` for each tool call in response
    /// - Emits `Frame::event(llm:result)` with usage stats and raw JSON
    /// - Emits `Frame::done` on completion
    /// - Returns `E_INVALID_ARGS` if actor missing, messages empty, or config invalid
    /// - Returns `E_INTERNAL` if kernel not initialized or LLM request fails
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Kernel & Actor Validation
        // =====================================================================
        // WHY: Check cancellation first to avoid wasted work. Validate kernel
        // and actor before expensive argument parsing.
        ctx.check_cancelled()?;

        // WHY: Kernel provides access to Store for persisting LLM request/response history.
        // Used by retry harness for debugging and analysis.
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        // WHY: Actor determines LLM configuration (model, temperature, max_tokens).
        // head/* uses powerful models (GPT-4), hand/* uses fast models (GPT-3.5).
        let actor = ctx
            .actor
            .as_deref()
            .ok_or_else(|| KernelError::invalid_args("actor is required for llm:chat"))?;
        let cfg = cfg_for_actor(actor)?;

        // =====================================================================
        // PHASE 2: Argument Parsing & Validation
        // =====================================================================
        // WHY: Validate messages/tools before expensive LLM invocation. Messages
        // must be non-empty array, tools are optional.
        let messages_v = data
            .get("messages")
            .cloned()
            .ok_or_else(|| KernelError::invalid_args("messages is required"))?;
        let messages: Vec<ChatMessage> = serde_json::from_value(messages_v)
            .map_err(|e| KernelError::invalid_args(format!("invalid messages: {e}")))?;
        if messages.is_empty() {
            return Err(KernelError::invalid_args("messages must not be empty"));
        }

        // WHY: Tools are optional. If provided, must be valid ToolSpec array.
        // Empty array means no tools available (LLM can only respond with text).
        let tools: Vec<ToolSpec> = match data.get("tools") {
            Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                .map_err(|e| KernelError::invalid_args(format!("invalid tools: {e}")))?,
            _ => Vec::new(),
        };

        // WHY: tool_choice controls tool selection strategy:
        // - "auto" (default): LLM decides whether to call tools
        // - "required": LLM MUST call at least one tool
        // - {"type": "function", "function": {"name": "fs:read"}}: Force specific tool
        let tool_choice = data.get("tool_choice").cloned().unwrap_or(serde_json::Value::Null);

        // =====================================================================
        // PHASE 3: Client Configuration & Retry Policy
        // =====================================================================
        // WHY: Retry policy uses exponential backoff with model fallback.
        // See RETRY LOGIC section in module docs for details.
        let policy = RetryPolicy::default_llm();

        // WHY: OpenAICompatClient abstracts API differences between providers.
        // Passes actor's config (base_url, api_key, model, temperature, max_tokens).
        let client = OpenAICompatClient::new(
            &cfg.base_url,
            &cfg.api_key,
            &cfg.model,
            cfg.temperature,
            cfg.max_tokens,
            cfg.extra_headers.clone(),
        );

        // =====================================================================
        // PHASE 4: LLM Invocation Begin Event
        // =====================================================================
        // WHY: Emit "llm:begin" event before invocation to signal start.
        // Enables UI to show loading spinner, log request metadata, etc.
        let _ = tx
            .send(
                Frame::event(
                    ctx.call_id,
                    json!({
                        "kind": "llm:begin",
                        "provider": cfg.provider,
                        "model": cfg.model,
                        "messages": messages.len(),
                        "tools": tools.len(),
                    }),
                )
                .with_actor(actor.to_string())
                .with_name("llm:chat"),
            )
            .await;

        // =====================================================================
        // PHASE 5: LLM Request with Retry Logic
        // =====================================================================
        // WHY: Delegate to retry harness for robust LLM invocation. Harness handles:
        // - Exponential backoff on 5xx errors, rate limits
        // - Model fallback on repeated failures
        // - Cancellation via ctx.cancel token
        // - Request/response persistence to Store
        //
        // RETRY CALLBACK: Spawns detached task to emit "llm:retry" events.
        // Must be detached to avoid blocking retry loop. Failures are silently
        // ignored (channel send failures are rare and non-critical for retries).
        let result: Result<ChatToolResult, crate::runtime::llm_harness::HarnessError> =
            chat_with_tools_retry_on_model(
                store.as_ref(),
                actor,
                &ctx.call_id.to_string(),
                0, // WHY: turn_index=0 (single-turn conversation for now)
                &client,
                None, // WHY: No system_override (use client default)
                messages,
                tools,
                tool_choice,
                policy,
                |attempt, note| {
                    // WHY: Clone values for detached task (move semantics)
                    let tx2 = tx.clone();
                    let actor2 = actor.to_string();
                    let call_id = ctx.call_id;
                    let note = note.to_string();
                    tokio::spawn(async move {
                        let _ = tx2
                            .send(
                                Frame::event(
                                    call_id,
                                    json!({
                                        "kind": "llm:retry",
                                        "attempt": attempt,
                                        "note": note,
                                    }),
                                )
                                .with_actor(actor2)
                                .with_name("llm:chat"),
                            )
                            .await;
                    });
                },
                Some(ctx.cancel.clone()),
            )
            .await;

        // =====================================================================
        // PHASE 6: Response Parsing & Frame Emission
        // =====================================================================
        // WHY: Convert LLM response into structured frames. Separate thinking
        // from visible content, emit tool calls individually, include raw JSON
        // for debugging.
        match result {
            Ok(res) => {
                // WHY: Parse thinking tags from content. Thinking is internal
                // reasoning, visible is user-facing output.
                let content = res.content.as_deref().unwrap_or("");
                let (thinking, visible) = parse_llm_content(content);

                // WHY: Emit thinking as separate frame so UI can render it
                // differently (e.g., collapsed accordion, grey text).
                if let Some(t) = thinking {
                    let _ = tx
                        .send(
                            Frame::item(
                                ctx.call_id,
                                json!({"type": "thinking", "content": t}),
                            )
                            .with_actor(actor.to_string())
                            .with_name("llm:chat"),
                        )
                        .await;
                }

                // WHY: Emit visible content as text_delta. Future work: stream
                // chunks incrementally for "typing animation" effect.
                if let Some(v) = visible {
                    let _ = tx
                        .send(
                            Frame::item(
                                ctx.call_id,
                                json!({"type": "text_delta", "content": v}),
                            )
                            .with_actor(actor.to_string())
                            .with_name("llm:chat"),
                        )
                        .await;
                }

                // WHY: Emit each tool call as separate frame. Enables incremental
                // tool execution (start executing first tool while parsing rest).
                // Parse arguments JSON, fallback to empty object on invalid JSON.
                for tc in res.tool_calls {
                    let arguments = serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                        .ok()
                        .filter(|v| v.is_object())
                        .unwrap_or_else(|| json!({}));
                    let _ = tx
                        .send(
                            Frame::item(
                                ctx.call_id,
                                json!({
                                    "type": "tool_call",
                                    "tool_call_id": tc.id,
                                    "name": tc.function.name,
                                    "arguments": arguments,
                                }),
                            )
                            .with_actor(actor.to_string())
                            .with_name("llm:chat"),
                        )
                        .await;
                }

                // WHY: Emit final result with usage stats and raw JSON. Usage stats
                // enable cost tracking, raw JSON enables debugging malformed responses.
                let _ = tx
                    .send(
                        Frame::event(
                            ctx.call_id,
                            json!({
                                "kind": "llm:result",
                                "usage": res.usage,
                                "request_json": res.request_json,
                                "response_json": res.response_json,
                            }),
                        )
                        .with_actor(actor.to_string())
                        .with_name("llm:chat"),
                    )
                    .await;

                // WHY: Frame::done signals completion. Enables caller to distinguish
                // between "still processing" and "done with empty response".
                let _ = tx.send(Frame::done(ctx.call_id)).await;
                Ok(())
            }
            Err(e) => {
                // WHY: Convert harness error to kernel error. Harness errors include
                // retry exhaustion, cancellation, API errors, JSON parsing failures.
                Err(KernelError::internal(e.message))
            }
        }
    }
}
