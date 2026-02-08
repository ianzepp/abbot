mod bundle;
mod config;

pub use bundle::{HandBundleBuilder, HandBundleConfig};
pub use config::HandConfig;

// HandService executes tasks assigned by heads.
//
// Hard cutover: hands use provider tool calls (no fenced parsing).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::ems::EmsHandle;
use crate::hal::llm::{LlmClient, UnifiedMessage as Message, UnifiedToolCall, UnifiedToolSpec};
use crate::history::Store;
use crate::kernel::{BatchCall, Frame, FrameOp};
use crate::runtime::Kernel;
use crate::syscalls::dispatch::dispatch_tool;

use crate::runtime::SnapshotManager;
use crate::runtime::llm_harness::{HarnessCtx, RetryPolicy, chat_with_tools_retry};

// =============================================================================
// SHARED HAND LOOP (used by both HandService and hand:run syscall)
// =============================================================================

/// Result of executing the hand loop.
pub struct HandResult {
    pub ok: bool,
    pub summary: String,
}

/// Execute the hand's LLM+tool loop as a standalone function.
///
/// This is the shared core used by both the `hand:run` syscall (direct invocation)
/// and the legacy HandService (task queue). It builds a hand bundle, runs the
/// LLM+tool loop, and returns a summary.
///
/// Uses the kernel dispatcher's `llm:chat` syscall for LLM calls, so no direct
/// LlmClient dependency is needed.
pub async fn execute_hand_loop(
    prompt: &str,
    context: &str,
    max_iters: usize,
    workspace: &Path,
    actor: &str,
    cancel: CancellationToken,
) -> HandResult {
    let Some(k) = Kernel::get() else {
        return HandResult {
            ok: false,
            summary: "kernel not initialized".to_string(),
        };
    };

    let store = match k.store() {
        Some(s) => s,
        None => {
            return HandResult {
                ok: false,
                summary: "kernel store not attached".to_string(),
            };
        }
    };

    let snapshot = SnapshotManager::new(workspace.to_path_buf(), Some(store.clone())).await;

    let snap = snapshot.get();
    let tools: Vec<crate::hal::llm::ToolSpec> = snap.hand_tools.clone();

    let run_id = Uuid::new_v4().to_string();
    let builder = HandBundleBuilder::new_with_snapshot(
        store.clone(),
        workspace.to_path_buf(),
        snapshot.clone(),
    );
    let traits = crate::runtime::AppConfig::global().traits.to_trait_names();
    let bundle_cfg = HandBundleConfig::new(&run_id, "system", prompt, context)
        .with_traits(traits)
        .with_max_iters(max_iters);
    let messages = builder.build(&bundle_cfg).await;

    // Convert UnifiedMessage bundle to ChatMessage for dispatcher-based LLM calls
    let mut chat_messages: Vec<crate::hal::llm::ChatMessage> = unified_to_chat_messages(messages);

    let mut vfs_cwd = String::from("/");
    let mut tool_failure_streak: usize = 0;

    for _iter in 0..max_iters {
        if cancel.is_cancelled() {
            return HandResult {
                ok: false,
                summary: "cancelled".to_string(),
            };
        }

        // Call LLM via kernel dispatcher (same pattern as room runner)
        let llm_result = match call_hand_llm(&chat_messages, &tools, actor, workspace).await {
            Ok(r) => r,
            Err(e) => {
                return HandResult {
                    ok: false,
                    summary: format!("LLM error: {e}"),
                };
            }
        };

        // No tool calls = agent is done
        if llm_result.tool_calls.is_empty() {
            let content = llm_result.content.unwrap_or_default();
            let ok = !content.trim().is_empty();
            return HandResult {
                ok,
                summary: if ok {
                    content.trim().to_string()
                } else {
                    "completed without output".to_string()
                },
            };
        }

        // Execute tool calls (one at a time, strict mode)
        chat_messages.push(crate::hal::llm::ChatMessage::assistant_tool_calls(
            llm_result.tool_calls.clone(),
        ));

        for tc in &llm_result.tool_calls {
            let out = dispatch_tool(
                &tc.function.name,
                &tc.function.arguments,
                actor,
                workspace,
                &vfs_cwd,
            )
            .await;

            // Update VFS CWD if fs:cd succeeded
            if tc.function.name == "tool__fs_cd"
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&out)
                && v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false)
                && let Some(new_cwd) = v
                    .get("data")
                    .and_then(|d| d.get("cwd"))
                    .and_then(|c| c.as_str())
            {
                vfs_cwd = new_cwd.to_string();
            }

            let success = tool_result_ok(&out);
            if success {
                tool_failure_streak = 0;
            } else {
                tool_failure_streak += 1;
            }

            chat_messages.push(crate::hal::llm::ChatMessage::tool_result(
                tc.id.clone(),
                out,
            ));
        }

        if tool_failure_streak >= 5 {
            return HandResult {
                ok: false,
                summary: "5 consecutive tool failures".to_string(),
            };
        }
    }

    HandResult {
        ok: false,
        summary: "iteration limit reached without final content".to_string(),
    }
}

/// Convert UnifiedMessage (client::Message) to ChatMessage (openai_compat::ChatMessage).
fn unified_to_chat_messages(
    messages: Vec<crate::hal::llm::UnifiedMessage>,
) -> Vec<crate::hal::llm::ChatMessage> {
    use crate::hal::llm::{ChatMessage, Role, UnifiedMessage};
    messages
        .into_iter()
        .map(|m| match m {
            UnifiedMessage::System(s) => ChatMessage::new(Role::System, s),
            UnifiedMessage::User(s) => ChatMessage::new(Role::User, s),
            UnifiedMessage::Assistant(s) => ChatMessage::new(Role::Assistant, s),
            UnifiedMessage::AssistantToolCalls(calls) => {
                let oai_calls: Vec<crate::hal::llm::ToolCall> = calls
                    .into_iter()
                    .map(|tc| crate::hal::llm::ToolCall {
                        id: tc.id,
                        call_type: "function".to_string(),
                        function: crate::hal::llm::ToolCallFunction {
                            name: tc.name,
                            arguments: tc.arguments.to_string(),
                        },
                    })
                    .collect();
                ChatMessage::assistant_tool_calls(oai_calls)
            }
            UnifiedMessage::ToolResult { id, content, .. } => ChatMessage::tool_result(id, content),
        })
        .collect()
}

/// Call the LLM via llm:chat syscall (dispatcher pattern, same as room runner).
async fn call_hand_llm(
    messages: &[crate::hal::llm::ChatMessage],
    tools: &[crate::hal::llm::ToolSpec],
    actor: &str,
    workspace: &Path,
) -> Result<HandLlmResult, String> {
    let Some(k) = Kernel::get() else {
        return Err("kernel not initialized".to_string());
    };
    let dispatcher = k.dispatcher().await;

    let payload = serde_json::json!({
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
    });

    let req = Frame::req("llm:chat", payload).with_actor(actor.to_string());
    let mut rx = dispatcher.dispatch(req, workspace.to_path_buf(), CancellationToken::new());

    let mut content = String::new();
    let mut tool_calls: Vec<crate::hal::llm::ToolCall> = Vec::new();

    while let Some(frame) = rx.recv().await {
        match frame.op {
            FrameOp::Item => {
                let Some(data) = frame.data else { continue };
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
                            .unwrap_or_else(|| serde_json::json!({}));
                        let arguments = serde_json::to_string(&arguments_v)
                            .ok()
                            .filter(|s| s.trim_start().starts_with('{'))
                            .unwrap_or_else(|| "{}".to_string());
                        if !id.is_empty() && !name.is_empty() {
                            let value = serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": {"name": name, "arguments": arguments}
                            });
                            if let Ok(tc) =
                                serde_json::from_value::<crate::hal::llm::ToolCall>(value)
                            {
                                tool_calls.push(tc);
                            }
                        }
                    }
                    _ => {}
                }
            }
            FrameOp::Error => {
                let msg = frame
                    .data
                    .as_ref()
                    .and_then(|d| d.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("llm error");
                return Err(msg.to_string());
            }
            FrameOp::Done => break,
            _ => {}
        }
    }

    Ok(HandLlmResult {
        content: if content.is_empty() {
            None
        } else {
            Some(content)
        },
        tool_calls,
    })
}

struct HandLlmResult {
    content: Option<String>,
    tool_calls: Vec<crate::hal::llm::ToolCall>,
}

// =============================================================================
// HAND SERVICE (legacy task queue worker)
// =============================================================================

pub struct HandService {
    store: Arc<Store>,
    hand_cfg: HandConfig,
    llm: Option<Arc<LlmClient>>,
    workspace_root: PathBuf,
    snapshot: Arc<SnapshotManager>,
    ems: Option<EmsHandle>,
    cancels: Arc<Mutex<HashMap<String, CancellationToken>>>,
    hand_id: String,
}

impl HandService {
    pub fn new(
        store: Arc<Store>,
        workspace_root: PathBuf,
        hand_id: impl Into<String>,
        snapshot: Arc<SnapshotManager>,
    ) -> Self {
        let hand_cfg = HandConfig::from_config();
        let llm = if hand_cfg.llm.enabled {
            Some(Arc::new(hand_cfg.llm.to_llm_client()))
        } else {
            None
        };

        Self {
            store,
            hand_cfg,
            llm,
            workspace_root,
            snapshot,
            ems: None,
            cancels: Arc::new(Mutex::new(HashMap::new())),
            hand_id: hand_id.into(),
        }
    }

    pub fn with_ems(mut self, ems: EmsHandle) -> Self {
        self.ems = Some(ems);
        self
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(self: Arc<Self>) {
        tracing::info!(hand = %self.hand_id, "hand service started");

        loop {
            let Some(task) = self.lease_task().await else {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            };

            self.run_one_task(task).await;
        }
    }

    async fn lease_task(&self) -> Option<TaskLease> {
        let k = Kernel::get()?;
        let dispatcher = k.dispatcher().await;
        let req = Frame::req("task:lease", serde_json::json!({"hand_id": self.hand_id}))
            .with_actor(format!("hand/{}", self.hand_id));

        let mut rx =
            dispatcher.dispatch(req, self.workspace_root.clone(), CancellationToken::new());
        let frame = rx.recv().await?;
        if frame.op != FrameOp::Ok {
            return None;
        }
        let v = frame.data?;

        let notify_scope = v
            .get("notify_scope")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let reply_to = v
            .get("reply_to")
            .and_then(|x| x.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        Some(TaskLease {
            task_id: v.get("task_id")?.as_str()?.to_string(),
            head_id: v
                .get("head_id")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown")
                .to_string(),
            scope: v
                .get("scope")
                .and_then(|x| x.as_str())
                .unwrap_or("main")
                .to_string(),
            prompt: v
                .get("prompt")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            input: v
                .get("input")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            notify_scope,
            reply_to,
            batch_calls: v.get("calls").and_then(|c| c.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let name = item.get("name")?.as_str()?.to_string();
                        let args = item.get("args").cloned().unwrap_or(serde_json::json!({}));
                        Some(BatchCall { name, args })
                    })
                    .collect()
            }),
        })
    }

    async fn run_one_task(&self, task: TaskLease) {
        let task_id = task.task_id.clone();
        let hand_id = self.hand_id.clone();

        let Some(llm) = self.llm.clone() else {
            self.complete_task(
                &task_id,
                &task.scope,
                task.notify_scope.as_deref(),
                task.reply_to,
                false,
                "FAILED: hand LLM disabled (configure hand.model and providers.<provider>.base_url in abbot.toml)".to_string(),
            )
            .await;
            return;
        };

        let cancel = CancellationToken::new();
        {
            let mut map = self.cancels.lock().expect("hand cancels lock poisoned");
            map.insert(task_id.clone(), cancel.clone());
        }

        struct CancelGuard {
            task_id: String,
            cancels: Arc<Mutex<HashMap<String, CancellationToken>>>,
        }

        impl Drop for CancelGuard {
            fn drop(&mut self) {
                let mut map = self.cancels.lock().expect("hand cancels lock poisoned");
                map.remove(&self.task_id);
            }
        }

        let _guard = CancelGuard {
            task_id: task_id.clone(),
            cancels: self.cancels.clone(),
        };

        run_hand_task(
            self.store.clone(),
            llm,
            self.hand_cfg.clone(),
            self.snapshot.clone(),
            self.workspace_root.clone(),
            task_id.clone(),
            task.head_id,
            hand_id,
            task.scope,
            task.notify_scope,
            task.reply_to,
            task.prompt,
            task.input,
            task.batch_calls,
            self.ems.clone(),
            cancel,
        )
        .await;
    }

    async fn complete_task(
        &self,
        task_id: &str,
        scope: &str,
        notify_scope: Option<&str>,
        reply_to: Option<Uuid>,
        ok: bool,
        summary: String,
    ) {
        let Some(k) = Kernel::get() else {
            return;
        };

        let completion_scope = notify_scope.unwrap_or(scope).trim();

        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "task:complete",
            serde_json::json!({
                "task_id": task_id,
                "ok": ok,
                "summary": summary,
                "scope": completion_scope,
                "notify_scope": notify_scope,
                "reply_to": reply_to.map(|u| u.to_string()),
            }),
        )
        .with_actor(format!("hand/{}", self.hand_id));
        let mut rx =
            dispatcher.dispatch(req, self.workspace_root.clone(), CancellationToken::new());
        let _ = rx.recv().await;
    }
}

#[derive(Debug, Clone)]
struct TaskLease {
    task_id: String,
    head_id: String,
    scope: String,
    prompt: String,
    input: String,
    notify_scope: Option<String>,
    reply_to: Option<Uuid>,
    batch_calls: Option<Vec<BatchCall>>,
}

#[allow(clippy::too_many_arguments)]
async fn run_hand_task(
    store: Arc<Store>,
    llm: Arc<LlmClient>,
    hand_cfg: HandConfig,
    snapshot: Arc<SnapshotManager>,
    workspace_root: PathBuf,
    task_id: String,
    head_id: String,
    hand_id: String,
    scope: String,
    notify_scope: Option<String>,
    reply_to: Option<Uuid>,
    prompt: String,
    input: String,
    batch_calls: Option<Vec<BatchCall>>,
    _ems: Option<EmsHandle>,
    cancel: CancellationToken,
) {
    #[allow(clippy::too_many_arguments)]
    async fn complete(
        task_id: &str,
        hand_id: &str,
        scope: &str,
        notify_scope: Option<&str>,
        reply_to: Option<Uuid>,
        ok: bool,
        summary: String,
        cwd: PathBuf,
    ) {
        let Some(k) = Kernel::get() else {
            return;
        };

        let completion_scope = notify_scope.unwrap_or(scope).trim();
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "task:complete",
            serde_json::json!({
                "task_id": task_id,
                "ok": ok,
                "summary": summary,
                "scope": completion_scope,
                "notify_scope": notify_scope,
                "reply_to": reply_to.map(|u| u.to_string()),
            }),
        )
        .with_actor(format!("hand/{hand_id}"));
        let mut rx = dispatcher.dispatch(req, cwd, CancellationToken::new());
        let _ = rx.recv().await;
    }
    let snap = snapshot.get();
    let tools: Vec<UnifiedToolSpec> = snap
        .hand_tools
        .iter()
        .map(|t| {
            UnifiedToolSpec::new(
                &t.function.name,
                t.function.description.as_deref().unwrap_or(""),
                t.function.parameters.clone(),
            )
        })
        .collect();

    let bundle_builder = HandBundleBuilder::new_with_snapshot(
        store.clone(),
        workspace_root.clone(),
        snapshot.clone(),
    );
    let traits = crate::runtime::AppConfig::global().traits.to_trait_names();
    let bundle_cfg = HandBundleConfig::new(&task_id, &head_id, &prompt, &input)
        .with_traits(traits)
        .with_max_iters(hand_cfg.max_iters);
    let mut messages = bundle_builder.build(&bundle_cfg).await;

    let tool_choice = serde_json::json!("auto");
    let policy = RetryPolicy::default_llm();
    let harness_ctx = HarnessCtx {
        provider: hand_cfg.llm.provider.clone(),
        model: hand_cfg.llm.model.clone(),
        base_url: hand_cfg.llm.base_url.clone(),
    };
    let dispatch_cwd = workspace_root;
    let mut vfs_cwd = String::from("/");

    // =========================================================================
    // Batch execution path: pre-execute tool calls, then hydrate the LLM context
    // =========================================================================
    if let Some(calls) = batch_calls {
        let mut fabricated_tool_calls = Vec::with_capacity(calls.len());
        let mut tool_results = Vec::with_capacity(calls.len());

        for (i, call) in calls.iter().enumerate() {
            let call_id = format!("batch_{}", i);
            let args_str = call.args.to_string();

            let start = std::time::Instant::now();
            let out = dispatch_tool(
                &call.name,
                &args_str,
                &format!("hand/{}", hand_id),
                &dispatch_cwd,
                &vfs_cwd,
            )
            .await;
            let duration_ms = start.elapsed().as_millis() as u64;
            let success = tool_result_ok(&out);

            let _ = store
                .log_hand_exec(
                    &task_id,
                    &hand_id,
                    0,
                    &call.name,
                    &args_str,
                    &out,
                    success,
                    duration_ms,
                    "batch",
                )
                .await;

            if !success {
                complete(
                    &task_id,
                    &hand_id,
                    &scope,
                    notify_scope.as_deref(),
                    reply_to,
                    false,
                    format!("FAILED: batch call {} ({}) failed: {}", i, call.name, out),
                    dispatch_cwd,
                )
                .await;
                return;
            }

            fabricated_tool_calls.push(UnifiedToolCall {
                id: call_id.clone(),
                name: call.name.clone(),
                arguments: call.args.clone(),
            });
            tool_results.push((call_id, out));
        }

        let system_msg = messages.remove(0);
        messages.clear();
        messages.push(system_msg);
        messages.push(Message::assistant_tool_calls(fabricated_tool_calls));
        for (call_id, result) in tool_results {
            messages.push(Message::tool_result(call_id, result));
        }
        messages.push(Message::user(prompt.clone()));
    }

    let mut tool_failure_streak: usize = 0;

    for iter in 0..hand_cfg.max_iters {
        if cancel.is_cancelled() {
            complete(
                &task_id,
                &hand_id,
                &scope,
                notify_scope.as_deref(),
                reply_to,
                false,
                "FAILED: task cancelled".to_string(),
                dispatch_cwd.clone(),
            )
            .await;
            return;
        }

        let res = match chat_with_tools_retry(
            store.as_ref(),
            "hand",
            &task_id,
            iter,
            llm.as_ref(),
            harness_ctx.clone(),
            messages.clone(),
            tools.clone(),
            tool_choice.clone(),
            policy.clone(),
            |attempt, note| {
                tracing::debug!(task_id, hand_id, iter, attempt, note, "llm retry");
            },
            Some(cancel.clone()),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let _ = store
                    .log_hand_exec(
                        &task_id,
                        &hand_id,
                        iter,
                        "_llm_error",
                        "",
                        &format!("llm failed after retries: {e}"),
                        false,
                        0,
                        "",
                    )
                    .await;
                complete(
                    &task_id,
                    &hand_id,
                    &scope,
                    notify_scope.as_deref(),
                    reply_to,
                    false,
                    format!("FAILED: llm failed after retries: {e}"),
                    dispatch_cwd.clone(),
                )
                .await;
                return;
            }
        };

        let _ = store
            .log_llm_interaction(
                "hand",
                &task_id,
                iter,
                &res.request_json,
                &res.response_json,
            )
            .await;

        if res.tool_calls.is_empty() {
            let content = res.content.unwrap_or_default();
            let ok = !content.trim().is_empty();
            complete(
                &task_id,
                &hand_id,
                &scope,
                notify_scope.as_deref(),
                reply_to,
                ok,
                if ok {
                    content.trim().to_string()
                } else {
                    "FAILED: model produced no tool calls and no final content".to_string()
                },
                dispatch_cwd.clone(),
            )
            .await;
            return;
        }

        if res.tool_calls.len() > 1 {
            let _ = store
                .log_hand_exec(
                    &task_id,
                    &hand_id,
                    iter,
                    "_warning",
                    "",
                    "warning: model returned multiple tool calls; only the first was executed",
                    true,
                    0,
                    "",
                )
                .await;
        }

        let tc = &res.tool_calls[0];
        messages.push(Message::assistant_tool_calls(vec![tc.clone()]));

        let args_str = tc.arguments.to_string();
        let start = std::time::Instant::now();
        let out = dispatch_tool(
            &tc.name,
            &args_str,
            &format!("hand/{}", hand_id),
            &dispatch_cwd,
            &vfs_cwd,
        )
        .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        if tc.name == "tool__fs_cd"
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&out)
            && v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false)
            && let Some(new_cwd) = v
                .get("data")
                .and_then(|d| d.get("cwd"))
                .and_then(|c| c.as_str())
        {
            vfs_cwd = new_cwd.to_string();
        }

        let success = tool_result_ok(&out);

        if success {
            tool_failure_streak = 0;
        } else {
            tool_failure_streak += 1;
        }
        let _ = store
            .log_hand_exec(
                &task_id,
                &hand_id,
                iter,
                &tc.name,
                &args_str,
                &out,
                success,
                duration_ms,
                "",
            )
            .await;

        let out_with_iter = format!(
            "{}\n\n[iteration {} of {}]",
            out,
            iter + 1,
            hand_cfg.max_iters
        );
        messages.push(Message::tool_result(tc.id.clone(), out_with_iter));

        if tool_failure_streak >= 5 {
            complete(
                &task_id,
                &hand_id,
                &scope,
                notify_scope.as_deref(),
                reply_to,
                false,
                "FAILED: 5 consecutive tool failures".to_string(),
                dispatch_cwd.clone(),
            )
            .await;
            return;
        }
    }

    complete(
        &task_id,
        &hand_id,
        &scope,
        notify_scope.as_deref(),
        reply_to,
        false,
        "FAILED: iteration limit reached without final content".to_string(),
        dispatch_cwd,
    )
    .await;
}

fn tool_result_ok(tool_result_json: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(tool_result_json) {
        Ok(v) => v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false),
        Err(_) => false,
    }
}
