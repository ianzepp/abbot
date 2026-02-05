use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelDispatcher, KernelError, Syscall, SyscallContext};
use crate::llm::{ChatMessage, ChatToolResult, OpenAICompatClient, ToolSpec};
use crate::runtime::{HandConfig, HeadConfig, Kernel, MindConfig};
use crate::runtime::llm_harness::{RetryPolicy, chat_with_tools_retry_on_model};

fn cfg_for_actor(actor: &str) -> Result<crate::runtime::Config, KernelError> {
    let a = actor.trim();
    let cfg = if a.starts_with("head/") {
        HeadConfig::from_config().llm
    } else if a.starts_with("hand/") {
        HandConfig::from_config().llm
    } else if a.starts_with("mind/") {
        MindConfig::from_config().llm
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
                let _ = tx
                    .send(
                        Frame::ok(
                            ctx.call_id,
                            json!({
                                "content": res.content,
                                "tool_calls": res.tool_calls,
                                "usage": res.usage,
                                "request_json": res.request_json,
                                "response_json": res.response_json,
                            }),
                        )
                        .with_actor(actor.to_string())
                        .with_name("llm:chat"),
                    )
                    .await;
                Ok(())
            }
            Err(e) => Err(KernelError::internal(e.message)),
        }
    }
}

pub fn register(dispatcher: &mut KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(LlmChat::new()));
}
