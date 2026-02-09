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
//! - **Tool calling support**: Passes tool specs to LLM, receives structured tool invocations
//! - **Streaming frames**: Emits thinking, text deltas, and tool calls incrementally
//! - **Request/response persistence**: Stores full LLM interactions in kernel Store
//! - **Cancellation support**: Respects parent context cancellation during retries/requests
//!
//! **Integration points:**
//! - `LlmClient` for provider-agnostic LLM API communication
//! - `llm_harness::chat_with_tools_retry` for retry orchestration
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

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::hal::llm::{ChatMessage, Role, UnifiedMessage as Message, UnifiedToolSpec as ToolSpec};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;
use crate::runtime::llm_harness::{HarnessCtx, RetryPolicy, chat_with_tools_retry};

use super::{cfg_for_actor, parse_llm_content};

/// Syscall for invoking LLMs with messages and tool calling.
pub struct LlmChat;

impl Default for LlmChat {
    fn default() -> Self {
        Self::new()
    }
}

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

        // Parse messages from caller (OpenAI ChatMessage wire format)
        let messages_v = data
            .get("messages")
            .cloned()
            .ok_or_else(|| KernelError::invalid_args("messages is required"))?;
        let chat_messages: Vec<ChatMessage> = serde_json::from_value(messages_v)
            .map_err(|e| KernelError::invalid_args(format!("invalid messages: {e}")))?;
        if chat_messages.is_empty() {
            return Err(KernelError::invalid_args("messages must not be empty"));
        }

        // Convert to unified Message format
        let messages = chat_messages_to_unified(chat_messages);

        // Parse tools — accept both unified flat format {name, description, parameters}
        // and OpenAI-compat wrapped format {type, function: {name, description, parameters}}.
        let tools: Vec<ToolSpec> = match data.get("tools") {
            Some(v) if !v.is_null() => parse_tool_specs(v.clone())
                .map_err(|e| KernelError::invalid_args(format!("invalid tools: {e}")))?,
            _ => Vec::new(),
        };

        let tool_choice = data
            .get("tool_choice")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        let policy = RetryPolicy::default_llm();
        let client = cfg.to_llm_client();
        let harness_ctx = HarnessCtx {
            provider: cfg.provider.clone(),
            model: cfg.model.clone(),
            base_url: cfg.base_url.clone(),
        };

        // Emit llm:begin event
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

        // LLM request with retry logic
        let result = chat_with_tools_retry(
            store.as_ref(),
            actor,
            &ctx.call_id.to_string(),
            0,
            &client,
            harness_ctx,
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

        // Response parsing & frame emission
        match result {
            Ok(res) => {
                crate::runtime::safe_mode::report_success();
                let content = res.content.as_deref().unwrap_or("");
                let (thinking, visible) = parse_llm_content(content);

                if let Some(t) = thinking {
                    let _ = tx
                        .send(
                            Frame::item(ctx.call_id, json!({"type": "thinking", "content": t}))
                                .with_actor(actor.to_string())
                                .with_name("llm:chat"),
                        )
                        .await;
                }

                if let Some(v) = visible {
                    let _ = tx
                        .send(
                            Frame::item(ctx.call_id, json!({"type": "text_delta", "content": v}))
                                .with_actor(actor.to_string())
                                .with_name("llm:chat"),
                        )
                        .await;
                }

                // Emit tool calls using unified ToolCall shape
                for tc in &res.tool_calls {
                    let arguments = if tc.arguments.is_object() {
                        tc.arguments.clone()
                    } else {
                        json!({})
                    };
                    let _ = tx
                        .send(
                            Frame::item(
                                ctx.call_id,
                                json!({
                                    "type": "tool_call",
                                    "tool_call_id": tc.id,
                                    "name": tc.name,
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
            Err(e) => {
                crate::runtime::safe_mode::report_failure();
                if crate::runtime::safe_mode::is_active() {
                    crate::runtime::safe_mode::wait_for_recovery(&ctx.cancel).await;
                }
                Err(KernelError::from(e))
            }
        }
    }
}

/// Parse tool specs from either unified flat format or OpenAI-compat wrapped format.
///
/// Unified: `[{name, description, parameters}, ...]`
/// OpenAI:  `[{type: "function", function: {name, description, parameters}}, ...]`
fn parse_tool_specs(v: serde_json::Value) -> Result<Vec<ToolSpec>, String> {
    // Try unified flat format first
    if let Ok(specs) = serde_json::from_value::<Vec<ToolSpec>>(v.clone()) {
        return Ok(specs);
    }

    // Fall back to OpenAI-compat wrapped format
    let arr = v.as_array().ok_or("tools must be an array")?;
    let mut specs = Vec::with_capacity(arr.len());
    for item in arr {
        let func = item
            .get("function")
            .ok_or("missing 'function' in tool spec")?;
        let name = func
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or("missing tool name")?;
        let desc = func
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        let params = func.get("parameters").cloned().unwrap_or(json!({}));
        specs.push(ToolSpec::new(name, desc, params));
    }
    Ok(specs)
}

/// Convert OpenAI-format `ChatMessage` list to unified `Message` list.
fn chat_messages_to_unified(msgs: Vec<ChatMessage>) -> Vec<Message> {
    use crate::hal::llm::UnifiedToolCall;

    msgs.into_iter()
        .map(|m| match m.role {
            Role::System => Message::System(m.content.unwrap_or_default()),
            Role::User => Message::User(m.content.unwrap_or_default()),
            Role::Assistant => {
                if let Some(tool_calls) = m.tool_calls {
                    let unified: Vec<UnifiedToolCall> = tool_calls
                        .into_iter()
                        .map(|tc| UnifiedToolCall {
                            id: tc.id,
                            name: tc.function.name,
                            arguments: serde_json::from_str(&tc.function.arguments)
                                .unwrap_or(serde_json::Value::Null),
                        })
                        .collect();
                    Message::AssistantToolCalls(unified)
                } else {
                    Message::Assistant(m.content.unwrap_or_default())
                }
            }
            Role::Tool => Message::ToolResult {
                id: m.tool_call_id.unwrap_or_default(),
                content: m.content.unwrap_or_default(),
                is_error: false,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::llm::{ToolCall, ToolCallFunction};

    #[test]
    fn converts_system_message() {
        let msgs = vec![ChatMessage::new(Role::System, "you are helpful")];
        let unified = chat_messages_to_unified(msgs);
        assert_eq!(unified.len(), 1);
        assert!(matches!(&unified[0], Message::System(s) if s == "you are helpful"));
    }

    #[test]
    fn converts_user_message() {
        let msgs = vec![ChatMessage::new(Role::User, "hello")];
        let unified = chat_messages_to_unified(msgs);
        assert_eq!(unified.len(), 1);
        assert!(matches!(&unified[0], Message::User(s) if s == "hello"));
    }

    #[test]
    fn converts_assistant_text() {
        let msgs = vec![ChatMessage::new(Role::Assistant, "hi there")];
        let unified = chat_messages_to_unified(msgs);
        assert_eq!(unified.len(), 1);
        assert!(matches!(&unified[0], Message::Assistant(s) if s == "hi there"));
    }

    #[test]
    fn converts_assistant_tool_calls() {
        let tc = ToolCall {
            id: "call_1".into(),
            call_type: "function".into(),
            function: ToolCallFunction {
                name: "fs:read".into(),
                arguments: r#"{"path":"/tmp"}"#.into(),
            },
        };
        let msgs = vec![ChatMessage::assistant_tool_calls(vec![tc])];
        let unified = chat_messages_to_unified(msgs);
        assert_eq!(unified.len(), 1);
        match &unified[0] {
            Message::AssistantToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_1");
                assert_eq!(calls[0].name, "fs:read");
                assert_eq!(calls[0].arguments["path"], "/tmp");
            }
            other => panic!("expected AssistantToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn converts_tool_result() {
        let msgs = vec![ChatMessage::tool_result("call_1", r#"{"ok":true}"#)];
        let unified = chat_messages_to_unified(msgs);
        assert_eq!(unified.len(), 1);
        match &unified[0] {
            Message::ToolResult {
                id,
                content,
                is_error,
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(content, r#"{"ok":true}"#);
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn converts_full_conversation() {
        let msgs = vec![
            ChatMessage::new(Role::System, "system prompt"),
            ChatMessage::new(Role::User, "do something"),
            ChatMessage::new(Role::Assistant, "ok I will"),
        ];
        let unified = chat_messages_to_unified(msgs);
        assert_eq!(unified.len(), 3);
        assert!(matches!(&unified[0], Message::System(_)));
        assert!(matches!(&unified[1], Message::User(_)));
        assert!(matches!(&unified[2], Message::Assistant(_)));
    }

    #[test]
    fn handles_invalid_tool_call_arguments_json() {
        let tc = ToolCall {
            id: "call_x".into(),
            call_type: "function".into(),
            function: ToolCallFunction {
                name: "test".into(),
                arguments: "not-valid-json".into(),
            },
        };
        let msgs = vec![ChatMessage::assistant_tool_calls(vec![tc])];
        let unified = chat_messages_to_unified(msgs);
        match &unified[0] {
            Message::AssistantToolCalls(calls) => {
                assert_eq!(calls[0].arguments, serde_json::Value::Null);
            }
            other => panic!("expected AssistantToolCalls, got {:?}", other),
        }
    }
}
