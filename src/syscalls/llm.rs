//! LLM Syscall - Provider-agnostic LLM communication
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `llm:chat` syscall provides a uniform interface for sending messages to
//! LLM providers and receiving structured responses. It is called by heads, hands,
//! and minds to perform reasoning and tool selection. The syscall refactor
//! established that `llm:chat` emits items (`thinking`, `text_delta`, `tool_call`)
//! rather than a single monolithic response, enabling callers to process each
//! piece as it arrives.
//!
//! WHY streaming items: Prior to the refactor, LLM responses returned as a single
//! `ok` payload, forcing callers to parse and distribute content (text vs thinking
//! vs tools). Emitting items separates concerns: thinking is logged but never
//! forwarded, text is sent to users, and tool calls are routed to internal or
//! external handlers.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Actor-driven configuration: The `actor` field determines which LLM config
//!   to use (head, hand, or mind), enabling different models/settings per role.
//! - Thinking extraction: `<thinking>` tags are parsed and emitted as a separate
//!   `thinking` item, preventing internal reasoning from leaking to users.
//! - Argument normalization: Tool call arguments are parsed from JSON string to
//!   object at this layer, ensuring downstream code receives structured data.
//! - Retry transparency: Emits `llm:retry` events so callers can observe retry
//!   attempts without blocking on the syscall response.
//!
//! TRADE-OFFS
//! ==========
//! - Thinking parsing happens here rather than in `llm` client. This duplicates
//!   regex logic but isolates the provider-specific client from Abbot-specific
//!   conventions like `<thinking>` tags.
//! - String-to-object argument conversion happens here. Alternative was to leave
//!   it to callers, but centralizing prevents divergence and ensures all tool
//!   calls have structured arguments.

use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelDispatcher, KernelError, Syscall, SyscallContext};
use crate::llm::{ChatMessage, ChatToolResult, OpenAICompatClient, ToolSpec};
use crate::runtime::{HandConfig, HeadConfig, Kernel, RoomConfig};
use crate::runtime::llm_harness::{RetryPolicy, chat_with_tools_retry_on_model};

// =============================================================================
// CONFIGURATION HELPERS
// =============================================================================
//
// WHY actor-driven: Heads, hands, and minds may use different models or settings
// (e.g., larger model for heads, smaller for hands). Actor prefix determines
// which config block to load.

fn cfg_for_actor(actor: &str) -> Result<crate::runtime::Config, KernelError> {
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
// THINKING EXTRACTION
// =============================================================================
//
// WHY here: The spec defines thinking as an Abbot-specific convention (use
// `<thinking>` tags for internal reasoning). Parsing happens at the syscall
// boundary so the provider client remains agnostic to Abbot semantics.
//
// TRADE-OFF: Regex parsing is simple but fragile. Nested or malformed tags may
// produce unexpected results. This is acceptable because thinking is informational
// (logged, not acted upon).

/// Parse LLM content into thinking and visible text.
///
/// WHY separate: Thinking is internal reasoning intended for logs but never
/// shown to users. Extracting it here prevents it from leaking into chat output.
fn parse_llm_content(content: &str) -> (Option<String>, Option<String>) {
    let thinking_re = Regex::new(r"<thinking>([\s\S]*?)</thinking>").unwrap();

    let mut thinking_parts = Vec::new();
    let mut visible_content = content.to_string();

    for cap in thinking_re.captures_iter(content) {
        thinking_parts.push(cap[1].to_string());
        visible_content = visible_content.replace(&cap[0], "");
    }

    let thinking = if thinking_parts.is_empty() {
        None
    } else {
        Some(thinking_parts.join("\n"))
    };

    let visible = visible_content.trim();
    let visible = if visible.is_empty() {
        None
    } else {
        Some(visible.to_string())
    };

    (thinking, visible)
}

// =============================================================================
// LLM:CHAT - LLM provider interaction
// =============================================================================
//
// WHY this exists: Provides a uniform interface for all LLM interactions,
// regardless of provider (OpenAI, Anthropic, local). Emits structured items
// (thinking, text, tool calls) enabling callers to handle each piece separately.

pub struct LlmChat;

impl LlmChat {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LlmChat {
    fn name(&self) -> &'static str {
        "llm:chat"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // -------------------------------------------------------------------------
        // SETUP: Validate actor and load configuration
        // WHY actor is required: Configuration (model, temperature, max_tokens)
        // varies by role (head/hand/mind). Without actor we cannot determine which
        // LLM settings to use.
        // -------------------------------------------------------------------------
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        let actor = ctx
            .actor
            .as_deref()
            .ok_or_else(|| KernelError::invalid_args("actor is required for llm:chat"))?;
        let cfg = cfg_for_actor(actor)?;

        let messages_v = data
            .get("messages")
            .cloned()
            .ok_or_else(|| KernelError::invalid_args("messages is required"))?;
        let messages: Vec<ChatMessage> = serde_json::from_value(messages_v)
            .map_err(|e| KernelError::invalid_args(format!("invalid messages: {e}")))?;
        if messages.is_empty() {
            return Err(KernelError::invalid_args("messages must not be empty"));
        }

        let tools: Vec<ToolSpec> = match data.get("tools") {
            Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                .map_err(|e| KernelError::invalid_args(format!("invalid tools: {e}")))?,
            _ => Vec::new(),
        };

        let tool_choice = data.get("tool_choice").cloned().unwrap_or(serde_json::Value::Null);

        // -------------------------------------------------------------------------
        // LLM INVOCATION: Call provider with retry policy
        // WHY retry policy: LLM providers may return transient errors (rate limits,
        // timeouts). Retrying with exponential backoff improves reliability.
        // -------------------------------------------------------------------------
        let policy = RetryPolicy::default_llm();

        let client = OpenAICompatClient::new(
            &cfg.base_url,
            &cfg.api_key,
            &cfg.model,
            cfg.temperature,
            cfg.max_tokens,
            cfg.extra_headers.clone(),
        );

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

        let result: Result<ChatToolResult, crate::runtime::llm_harness::HarnessError> =
            chat_with_tools_retry_on_model(
                store.as_ref(),
                actor,
                &ctx.call_id.to_string(),
                0,
                &client,
                None,
                messages,
                tools,
                tool_choice,
                policy,
                |attempt, note| {
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

        // -------------------------------------------------------------------------
        // RESPONSE EMISSION: Parse and emit structured items
        // WHY separate items: Thinking must be logged but never forwarded to user.
        // Text goes to user. Tool calls are routed to handlers. Emitting each as
        // a separate item enables callers to handle them distinctly.
        // -------------------------------------------------------------------------
        match result {
            Ok(res) => {
                let content = res.content.as_deref().unwrap_or("");
                let (thinking, visible) = parse_llm_content(content);

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

                // WHY normalize arguments: Some providers return stringified JSON,
                // others return objects. Parsing here ensures callers always receive
                // structured arguments.
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

                let _ = tx.send(Frame::done(ctx.call_id)).await;
                Ok(())
            }
            Err(e) => Err(KernelError::internal(e.message)),
        }
    }
}

// =============================================================================
// LLM:CHAOS - Random trait combination generator
// =============================================================================
//
// WHY this exists: Generates random agent personalities by rolling across all
// trait axes. Supports pinning specific axes while randomizing the rest,
// enabling genetic-algorithm-style breeding of agent personalities.

pub struct LlmChaos;

impl LlmChaos {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LlmChaos {
    fn name(&self) -> &'static str {
        "llm:chaos"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        // Parse optional pinned axes: { "pin": { "fever": "meth", "ego": "torvalds" } }
        let pinned: std::collections::HashMap<String, String> = data
            .get("pin")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        // Parse optional excluded axes: { "exclude": ["filter", "poverty"] }
        let exclude: Vec<String> = data
            .get("exclude")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let result = crate::runtime::chaos::roll(&pinned, &exclude);

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "traits": result.selections,
                    "prompt": result.prompt,
                }),
            ))
            .await;

        Ok(())
    }
}

// =============================================================================
// LLM:CHAOS:LIST - List available trait axes and levels
// =============================================================================

pub struct LlmChaosList;

impl LlmChaosList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for LlmChaosList {
    fn name(&self) -> &'static str {
        "llm:chaos:list"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        _data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let axes = crate::runtime::chaos::list_axes();
        let map: serde_json::Map<String, serde_json::Value> = axes
            .into_iter()
            .map(|(name, levels)| {
                (
                    name.to_string(),
                    json!(levels),
                )
            })
            .collect();

        let _ = tx
            .send(Frame::ok(ctx.call_id, serde_json::Value::Object(map)))
            .await;

        Ok(())
    }
}

// =============================================================================
// REGISTRATION
// =============================================================================

pub fn register(dispatcher: &mut KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(LlmChat::new()));
    dispatcher.register(Arc::new(LlmChaos::new()));
    dispatcher.register(Arc::new(LlmChaosList::new()));
}
