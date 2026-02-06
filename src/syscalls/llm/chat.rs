use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::llm::{ChatMessage, ChatToolResult, OpenAICompatClient, ToolSpec};
use crate::runtime::Kernel;
use crate::runtime::llm_harness::{RetryPolicy, chat_with_tools_retry_on_model};

use super::{cfg_for_actor, parse_llm_content};

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
