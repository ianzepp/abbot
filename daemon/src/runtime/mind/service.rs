//! Mind Loop Service
//!
//! A proactive single-agent observer that wakes on a fixed cadence to review
//! system activity in #main and decide whether to act. Fundamentally different
//! from RoomCoordinator (multi-agent deliberation) and HeadService (reactive
//! need processing).
//!
//! The mind loop asks "what could I do?" — not "what is assigned to me?"

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::hal::llm::{ChatMessage, ToolCall, ToolSpec};
use crate::kernel::{Frame, FrameOp};
use crate::runtime::Kernel;
use crate::history::Store;
use crate::syscalls::dispatch::{dispatch_tool, mind_loop_catalog};

use super::bundle::{MindLoopBundleBuilder, MindLoopBundleConfig};
use super::config::MindLoopConfig;

/// Proactive mind loop observer service.
///
/// Follows the `Arc<Self> + start() + run()` pattern. Wakes on a fixed cadence,
/// builds context, calls the LLM, and dispatches tool calls until noop or
/// max_rounds.
pub struct MindLoop {
    store: Arc<Store>,
    workspace: PathBuf,
}

impl MindLoop {
    pub fn new(store: Arc<Store>, workspace: PathBuf) -> Self {
        Self { store, workspace }
    }

    /// Spawn the mind loop as a background task.
    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(&self) {
        let cfg = MindLoopConfig::from_config();

        tracing::info!(
            cadence_secs = cfg.cadence_secs,
            max_rounds = cfg.max_rounds,
            channel = %cfg.channel,
            "mind loop started"
        );

        let mut last_wake_ts: Option<i64> = None;

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(cfg.cadence_secs)).await;

            let now_ms = now_ms();
            tracing::debug!("mind loop waking");

            match self.wake_cycle(&cfg, last_wake_ts).await {
                Ok(()) => {
                    tracing::debug!("mind loop wake cycle complete");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "mind loop wake cycle failed");
                }
            }

            last_wake_ts = Some(now_ms);
        }
    }

    async fn wake_cycle(
        &self,
        cfg: &MindLoopConfig,
        last_wake_ts: Option<i64>,
    ) -> Result<(), String> {
        // Build context
        let builder = MindLoopBundleBuilder::new(self.store.clone());
        let bundle_cfg = MindLoopBundleConfig::new(&cfg.channel, self.workspace.clone())
            .with_last_wake_ts(last_wake_ts)
            .with_max_context_items(cfg.max_context_items);
        let mut messages = builder.build(&bundle_cfg);

        let tools = mind_loop_catalog();
        let actor = format!("mind/{}", cfg.channel);

        // Multi-round tool loop
        for round in 0..cfg.max_rounds {
            tracing::debug!(round, "mind loop LLM call");

            let result = self
                .call_llm(&messages, &tools, &actor)
                .await?;

            let tool_calls = result.tool_calls;
            let content = result.content;

            // No tool calls — text-only response or empty, we're done
            if tool_calls.is_empty() {
                if let Some(text) = &content {
                    tracing::debug!(text = %text, "mind loop text response (no tools)");
                }
                break;
            }

            // Check for noop signal
            let has_noop = tool_calls
                .iter()
                .any(|tc| tc.function.name == "tool__noop_signal");

            if has_noop {
                let reason = tool_calls
                    .iter()
                    .find(|tc| tc.function.name == "tool__noop_signal")
                    .and_then(|tc| {
                        serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                            .ok()
                            .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(|s| s.to_string()))
                    })
                    .unwrap_or_default();

                tracing::debug!(reason = %reason, "mind loop noop");
                break;
            }

            // Add assistant message with tool calls to conversation
            messages.push(ChatMessage::assistant_tool_calls(tool_calls.clone()));

            // Dispatch each tool call and collect results
            for tc in &tool_calls {
                tracing::debug!(
                    tool = %tc.function.name,
                    round,
                    "mind loop dispatching tool"
                );

                let out = dispatch_tool(
                    &tc.function.name,
                    &tc.function.arguments,
                    &actor,
                    &self.workspace,
                )
                .await;

                messages.push(ChatMessage::tool_result(tc.id.clone(), out));
            }
        }

        Ok(())
    }

    /// Call the LLM via llm:chat syscall and collect streamed response.
    async fn call_llm(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        actor: &str,
    ) -> Result<LlmResult, String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;

        let payload = json!({
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
        });

        let req = Frame::req("llm:chat", payload).with_actor(actor.to_string());
        let mut rx = dispatcher.dispatch(
            req,
            self.workspace.clone(),
            tokio_util::sync::CancellationToken::new(),
        );

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        while let Some(frame) = rx.recv().await {
            match frame.op {
                FrameOp::Item => {
                    let Some(data) = frame.data else {
                        continue;
                    };
                    match data.get("type").and_then(|v| v.as_str()) {
                        Some("text_delta") => {
                            if let Some(text) = data.get("content").and_then(|v| v.as_str()) {
                                content.push_str(text);
                            }
                        }
                        Some("tool_call") => {
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
                                .unwrap_or_else(|| json!({}));
                            let arguments = serde_json::to_string(&arguments_v)
                                .ok()
                                .filter(|s| s.trim_start().starts_with('{'))
                                .unwrap_or_else(|| "{}".to_string());
                            if !id.is_empty() && !name.is_empty() {
                                let value = json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {"name": name, "arguments": arguments}
                                });
                                if let Ok(tc) = serde_json::from_value::<ToolCall>(value) {
                                    tool_calls.push(tc);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                FrameOp::Done => break,
                FrameOp::Error => {
                    let msg = frame
                        .data
                        .as_ref()
                        .and_then(|d| d.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("llm syscall error")
                        .to_string();
                    return Err(msg);
                }
                _ => {}
            }
        }

        let content = if content.trim().is_empty() {
            None
        } else {
            Some(content)
        };

        Ok(LlmResult {
            content,
            tool_calls,
        })
    }
}

struct LlmResult {
    content: Option<String>,
    tool_calls: Vec<ToolCall>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
