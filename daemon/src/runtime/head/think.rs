use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde_json::json;

use super::HeadService;
use super::config::{head_context_budget_tokens, head_time_gap_marker_minutes, load_tars_dials};
use super::types::{ActiveNeed, WaitKind};
use crate::Scope;
use crate::hal::llm::ToolCall;
use crate::runtime::summarize_tool_args;
use crate::runtime::{HeadBundleBuilder, HeadBundleConfig, Kernel};
use crate::syscalls::dispatch::{ToolEffect, dispatch_tool, tool_effect};

impl HeadService {
    /// Main thinking loop: call LLM, execute tools, emit chat, handle external calls.
    pub(super) async fn think(
        &self,
        need: &mut ActiveNeed,
    ) -> (String, Option<WaitKind>, Vec<String>) {
        let Some(_llm) = &self.llm else {
            return ("LLM not configured".to_string(), None, Vec::new());
        };

        let snap = self.snapshot.get();
        let tools = snap.head_tools.clone();
        let bundle_builder = HeadBundleBuilder::new_with_snapshot(
            self.store.clone(),
            self.workspace_root.clone(),
            self.snapshot.clone(),
        );

        let mut scopes = self.scopes.clone();
        if let Some(ref s) = need.scope {
            let s = s.trim();
            if !s.is_empty() {
                let sc = Scope::from(s);
                if !scopes.contains(&sc) {
                    scopes.push(sc);
                }
            }
        }

        let tars = load_tars_dials(&self.workspace_root);
        let bundle_cfg = HeadBundleConfig::new(&self.head_id, scopes)
            .with_context_budget_tokens(head_context_budget_tokens())
            .with_time_gap_marker_minutes(head_time_gap_marker_minutes())
            .with_traits(self.traits.clone())
            .with_tars(tars);
        // Build the initial transcript once per need; resumes continue from `need.llm_messages`.
        if need.llm_messages.is_empty() {
            let mut messages = bundle_builder.build(&bundle_cfg).await;

            // Inject the need as a user message.
            let need_prompt = format!(
                "You have been assigned a need to address:\n\n{}\n\nContext: {}",
                need.need_text,
                if need.context.is_empty() {
                    "(none)"
                } else {
                    &need.context
                }
            );
            messages.push(crate::hal::llm::ChatMessage::new(
                crate::hal::llm::Role::User,
                need_prompt,
            ));
            need.llm_messages = messages;
        }

        tracing::debug!(
            head = %self.head_id,
            need_id = %need.need_id,
            message_count = need.llm_messages.len(),
            "thinking"
        );

        let default_scope = need
            .scope
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| self.scopes.first().map(|s| s.to_string()))
            .unwrap_or_else(|| "main".to_string());

        let run_id = format!("need:{}", need.need_id);
        let reply_to = need.reply_to;

        let tool_choice = serde_json::json!("auto");

        let mut final_summary = String::new();
        let mut wait_kind: Option<WaitKind> = None;
        let mut pending_task_ids: Vec<String> = Vec::new();

        let mut tools = tools;

        let (external_tools, external_names) = {
            let mut external_tools: Vec<crate::hal::llm::ToolSpec> = Vec::new();
            let mut external_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();

            if let Ok(ext) = self.store.list_tools(&default_scope, "external").await {
                for t in ext {
                    if let Ok(schema) = serde_json::from_str::<serde_json::Value>(&t.schema_json) {
                        let internal_name = format!("user__{}", t.name);
                        external_tools.push(crate::hal::llm::ToolSpec::function(
                            internal_name.clone(),
                            t.summary.clone(),
                            schema,
                        ));
                        external_names.insert(internal_name);
                    }
                }
            }

            (external_tools, external_names)
        };

        tools.extend(external_tools.iter().cloned());

        // WHY 12 rounds: Balances reasoning depth with cost. Most tasks complete
        // in 2-4 rounds; 12 provides headroom for complex multi-step reasoning.
        for iter in 0..12usize {
            if self.is_turn_cancelled(need).await {
                final_summary = "Cancelled".to_string();
                break;
            }

            // -------------------------------------------------------------------------
            // PHASE 1: LLM CALL
            // -------------------------------------------------------------------------
            let result = match self
                .chat_head_llm_with_fallback(
                    &default_scope,
                    need.llm_messages.clone(),
                    tools.clone(),
                    tool_choice.clone(),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(head = %self.head_id, error = %e.message, "head llm failed after retries");
                    final_summary = format!("LLM error: {}", e.message);

                    if reply_to.is_some() && !self.is_turn_cancelled(need).await {
                        self.send_error(need, &final_summary).await;
                    }
                    break;
                }
            };

            let _ = self
                .store
                .log_llm_interaction(
                    "head",
                    &run_id,
                    iter,
                    &result.request_json,
                    &result.response_json,
                )
                .await;

            // -------------------------------------------------------------------------
            // PHASE 2: LOG AND EMIT RESPONSE
            // -------------------------------------------------------------------------
            if !result.tool_calls.is_empty() {
                for tc in &result.tool_calls {
                    tracing::info!(
                        head = %self.head_id,
                        iter,
                        tool = %tc.function.name,
                        args = ?summarize_tool_args(&tc.function.name, &tc.function.arguments),
                        scope = %default_scope,
                        reply_to = ?reply_to,
                        "head tool call"
                    );
                }
            }
            if let Some(ref content) = result.content
                && !content.trim().is_empty()
            {
                tracing::info!(
                    head = %self.head_id,
                    scope = %default_scope,
                    reply_to = ?reply_to,
                    content = %truncate(content, 100),
                    "head says"
                );
            }

            // -------------------------------------------------------------------------
            // PHASE 3: HANDLE TOOL CALLS
            // -------------------------------------------------------------------------
            if !result.tool_calls.is_empty() {
                if let Some(ref content) = result.content
                    && !content.trim().is_empty()
                {
                    need.llm_messages.push(crate::hal::llm::ChatMessage::new(
                        crate::hal::llm::Role::Assistant,
                        content.clone(),
                    ));
                    if let Some(reply_to) = reply_to
                        && !self.is_turn_cancelled(need).await
                    {
                        let _ = self
                            .emit_chat_message(default_scope.as_str(), reply_to, content)
                            .await;
                    }
                }

                let mut external_calls: Vec<ToolCall> = result
                    .tool_calls
                    .iter()
                    .filter(|tc| external_names.contains(&tc.function.name))
                    .cloned()
                    .collect();

                if !external_calls.is_empty() {
                    const MAX_EXTERNAL_CALLS_PER_TURN: usize = 8;
                    if external_calls.len() > MAX_EXTERNAL_CALLS_PER_TURN {
                        external_calls.truncate(MAX_EXTERNAL_CALLS_PER_TURN);
                    }

                    external_calls.retain(|tc| {
                        let sig = external_tool_sig(tc);
                        !need.recent_external_sigs.iter().any(|s| *s == sig)
                    });

                    if external_calls.is_empty() {
                        final_summary =
                            "Requested external tools, but all were recently repeated; refusing to re-run.".to_string();

                        if let Some(r) = reply_to
                            && !self.is_turn_cancelled(need).await
                        {
                            let _ = self
                                .emit_chat_message(default_scope.as_str(), r, &final_summary)
                                .await;
                            let _ = self
                                .emit_chat_done(default_scope.as_str(), r, "complete")
                                .await;
                        }
                        break;
                    }

                    need.llm_messages
                        .push(crate::hal::llm::ChatMessage::assistant_tool_calls(
                            external_calls.clone(),
                        ));

                    for tc in &external_calls {
                        let sig = external_tool_sig(tc);
                        need.recent_external_sigs.push_back(sig);
                        const MAX_SIGS: usize = 64;
                        while need.recent_external_sigs.len() > MAX_SIGS {
                            need.recent_external_sigs.pop_front();
                        }
                    }

                    need.pending_external = external_calls.clone();

                    let Some(parent_id) = reply_to else {
                        for tc in &external_calls {
                            need.llm_messages
                                .push(crate::hal::llm::ChatMessage::tool_result(
                                tc.id.clone(),
                                "Requested external tool, but missing reply_to for correlation."
                                    .to_string(),
                            ));
                        }
                        need.pending_external.clear();
                        continue;
                    };

                    if self.is_turn_cancelled(need).await {
                        final_summary = "Cancelled".to_string();
                        break;
                    }

                    let mut dispatch_failed = false;
                    for tc in &external_calls {
                        let arguments =
                            serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                                .ok()
                                .filter(|v| v.is_object())
                                .unwrap_or_else(|| json!({}));
                        let client_name = tc
                            .function
                            .name
                            .strip_prefix("user__")
                            .unwrap_or(tc.function.name.as_str());
                        if let Err(e) = self
                            .emit_chat_tool(
                                default_scope.as_str(),
                                parent_id,
                                &tc.id,
                                client_name,
                                &arguments,
                            )
                            .await
                        {
                            need.llm_messages
                                .push(crate::hal::llm::ChatMessage::tool_result(
                                    tc.id.clone(),
                                    format!("External tool dispatch failed: {e}"),
                                ));
                            need.pending_external.clear();
                            dispatch_failed = true;
                            break;
                        }
                    }

                    if dispatch_failed {
                        continue;
                    }

                    let _ = self
                        .emit_chat_done(default_scope.as_str(), parent_id, "awaiting_tools")
                        .await;
                    wait_kind = Some(WaitKind::ExternalTool);
                    final_summary = "Requested external tool(s); waiting for result.".to_string();
                    break;
                }

                need.llm_messages
                    .push(crate::hal::llm::ChatMessage::assistant_tool_calls(
                        result.tool_calls.clone(),
                    ));

                for tc in &result.tool_calls {
                    if external_names.contains(&tc.function.name) {
                        continue;
                    }
                    if tc.function.name == "tool__task_create" {
                        wait_kind = Some(WaitKind::Tasks);
                    }

                    let is_mutating = tool_effect(&tc.function.name)
                        .map(|e| e == ToolEffect::Mutating)
                        .unwrap_or(false);
                    let _write_guard = if is_mutating {
                        Some(self.session_locks.acquire(&default_scope).await)
                    } else {
                        None
                    };

                    let out = dispatch_tool(
                        &tc.function.name,
                        &tc.function.arguments,
                        &format!("head/{}", self.head_id),
                        &self.workspace_root,
                    )
                    .await;

                    if tc.function.name == "tool__task_create"
                        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&out)
                        && v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false)
                        && let Some(task_id) = v
                            .get("data")
                            .and_then(|d| d.get("task_id"))
                            .and_then(|t| t.as_str())
                        && !pending_task_ids.iter().any(|id| id == task_id)
                    {
                        pending_task_ids.push(task_id.to_string());
                    }

                    need.llm_messages
                        .push(crate::hal::llm::ChatMessage::tool_result(
                            tc.id.clone(),
                            out,
                        ));
                }

                if wait_kind == Some(WaitKind::Tasks) {
                    final_summary = "Queued tasks; waiting for completion.".to_string();
                    break;
                }
                continue;
            }

            // -------------------------------------------------------------------------
            // PHASE 4: FINAL RESPONSE (NO TOOL CALLS)
            // -------------------------------------------------------------------------
            let content = result.content.unwrap_or_default();
            if !content.trim().is_empty() {
                need.llm_messages.push(crate::hal::llm::ChatMessage::new(
                    crate::hal::llm::Role::Assistant,
                    content.clone(),
                ));
                if let Some(r) = reply_to
                    && !self.is_turn_cancelled(need).await
                {
                    let _ = self
                        .emit_chat_message(default_scope.as_str(), r, &content)
                        .await;
                    let _ = self
                        .emit_chat_done(default_scope.as_str(), r, "complete")
                        .await;
                }

                final_summary = truncate(&content, 200);
            } else {
                final_summary = "Completed without response".to_string();

                if let Some(r) = reply_to
                    && !self.is_turn_cancelled(need).await
                {
                    let _ = self
                        .emit_chat_done(default_scope.as_str(), r, "complete")
                        .await;
                }
            }
            break;
        }

        (final_summary, wait_kind, pending_task_ids)
    }

    /// Call the LLM via llm:chat syscall and collect streamed response.
    async fn chat_head_llm_with_fallback(
        &self,
        scope: &str,
        messages: Vec<crate::hal::llm::ChatMessage>,
        tools: Vec<crate::hal::llm::ToolSpec>,
        tool_choice: serde_json::Value,
    ) -> Result<crate::hal::llm::ChatToolResult, crate::runtime::llm_harness::HarnessError> {
        let Some(k) = Kernel::get() else {
            return Err(crate::runtime::llm_harness::HarnessError {
                message: "kernel not initialized".to_string(),
            });
        };
        let dispatcher = k.dispatcher().await;
        let _ = scope;

        let payload = serde_json::json!({
            "messages": messages,
            "tools": tools,
            "tool_choice": tool_choice,
        });

        let req = crate::kernel::Frame::req("llm:chat", payload)
            .with_actor(format!("head/{}", self.head_id));
        let mut rx = dispatcher.dispatch(
            req,
            self.workspace_root.clone(),
            tokio_util::sync::CancellationToken::new(),
        );

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut usage: Option<crate::hal::llm::Usage> = None;
        let mut request_json = String::new();
        let mut response_json = String::new();

        while let Some(frame) = rx.recv().await {
            match frame.op {
                crate::kernel::FrameOp::Item => {
                    let Some(data) = frame.data else {
                        continue;
                    };
                    match data.get("type").and_then(|v| v.as_str()) {
                        Some("text_delta") => {
                            if let Some(text) = data.get("content").and_then(|v| v.as_str()) {
                                content.push_str(text);
                            }
                        }
                        Some("thinking") => {
                            // Persisted centrally by the dispatcher's FrameStore.
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
                            let arguments_v =
                                data.get("arguments").cloned().unwrap_or_else(|| json!({}));
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
                crate::kernel::FrameOp::Event => {
                    if let Some(data) = frame.data.as_ref()
                        && data.get("kind").and_then(|v| v.as_str()) == Some("llm:result")
                    {
                        if let Some(u) = data.get("usage") {
                            let parsed: Option<crate::hal::llm::Usage> =
                                serde_json::from_value(u.clone()).ok();
                            if parsed.is_some() {
                                usage = parsed;
                            }
                        }
                        if let Some(req) = data.get("request_json").and_then(|v| v.as_str()) {
                            request_json = req.to_string();
                        }
                        if let Some(resp) = data.get("response_json").and_then(|v| v.as_str()) {
                            response_json = resp.to_string();
                        }
                    }
                }
                crate::kernel::FrameOp::Done => break,
                crate::kernel::FrameOp::Error => {
                    let msg = frame
                        .data
                        .as_ref()
                        .and_then(|d| d.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("llm syscall error")
                        .to_string();

                    return Err(crate::runtime::llm_harness::HarnessError { message: msg });
                }
                _ => {}
            }
        }

        let content = if content.trim().is_empty() {
            None
        } else {
            Some(content)
        };

        Ok(crate::hal::llm::ChatToolResult {
            content,
            tool_calls,
            usage,
            request_json,
            response_json,
        })
    }
}

/// Compute a signature for an external tool call (name + arguments hash).
fn external_tool_sig(tc: &ToolCall) -> u64 {
    let mut hasher = DefaultHasher::new();
    tc.function.name.hash(&mut hasher);
    tc.function.arguments.hash(&mut hasher);
    hasher.finish()
}

/// Truncate a string for logging (preserves valid UTF-8 boundaries).
pub(super) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }

    let mut cut = max.min(s.len());
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let prefix = &s[..cut];
    format!("{}...", prefix)
}
