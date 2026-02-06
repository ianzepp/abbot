mod bundle;
mod config;

pub use bundle::{AutistMode, HandBundleBuilder, HandBundleConfig};
pub use config::HandConfig;

// HandService executes tasks assigned by heads.
//
// Hard cutover: hands use provider tool calls (no fenced parsing).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent_tools::{SharedCwd, Workspace}; // kept for plugin dispatch path
use crate::syscalls::dispatch::dispatch_tool;
use crate::ems::EmsHandle;
use crate::history::Store;
use crate::kernel::{BatchCall, Frame, FrameOp};
use crate::llm::{ChatMessage, OpenAICompatClient, Role, ToolCall, ToolCallFunction};
use crate::runtime::Kernel;

use crate::runtime::llm_harness::{RetryPolicy, chat_with_tools_retry};
use crate::runtime::SnapshotManager;

const MAX_CONCURRENT_TASKS: usize = 8;

pub struct HandService {
    store: Arc<Store>,
    hand_cfg: HandConfig,
    llm: Option<Arc<OpenAICompatClient>>,
    workspace_root: PathBuf,
    snapshot: Arc<SnapshotManager>,
    task_semaphore: Arc<Semaphore>,
    autist: AutistMode,
    filter: crate::runtime::FilterMode,
    poverty: crate::runtime::PovertyMode,
    ems: Option<EmsHandle>,
    cancels: Arc<Mutex<HashMap<String, CancellationToken>>>,
    hand_id: String,
}

impl HandService {
    pub fn new(store: Arc<Store>, workspace_root: PathBuf, snapshot: Arc<SnapshotManager>) -> Self {
        let hand_cfg = HandConfig::from_config();
        let autist = hand_cfg.autist.clone();
        let filter = hand_cfg.filter.clone();
        let poverty = hand_cfg.poverty.clone();
        let llm = if hand_cfg.llm.enabled {
            Some(Arc::new(OpenAICompatClient::new(
                hand_cfg.llm.base_url.clone(),
                hand_cfg.llm.api_key.clone(),
                hand_cfg.llm.model.clone(),
                hand_cfg.llm.temperature,
                hand_cfg.llm.max_tokens,
                hand_cfg.llm.extra_headers.clone(),
            )))
        } else {
            None
        };

        Self {
            store,
            hand_cfg,
            llm,
            workspace_root,
            snapshot,
            task_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS)),
            autist,
            filter,
            poverty,
            ems: None,
            cancels: Arc::new(Mutex::new(HashMap::new())),
            hand_id: "hand-0".to_string(),
        }
    }

    pub fn with_autist(mut self, autist: AutistMode) -> Self {
        self.autist = autist;
        self
    }

    pub fn with_filter(mut self, filter: crate::runtime::FilterMode) -> Self {
        self.filter = filter;
        self
    }

    pub fn with_poverty(mut self, poverty: crate::runtime::PovertyMode) -> Self {
        self.poverty = poverty;
        self
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
            let permit = match self.task_semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };

            let Some(task) = self.lease_task().await else {
                drop(permit);
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            };

            let this = self.clone();
            tokio::spawn(async move {
                this.run_one_task(task, permit).await;
            });
        }
    }

    async fn lease_task(&self) -> Option<TaskLease> {
        let Some(k) = Kernel::get() else {
            return None;
        };
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

    async fn run_one_task(&self, task: TaskLease, _permit: OwnedSemaphorePermit) {
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
            Workspace::new(self.workspace_root.clone()),
            task_id.clone(),
            task.head_id,
            hand_id,
            task.scope,
            task.notify_scope,
            task.reply_to,
            task.prompt,
            task.input,
            task.batch_calls,
            self.autist.clone(),
            self.filter.clone(),
            self.poverty.clone(),
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

        // Task completions should be indexed under the conversation scope so heads can
        // incorporate them in subsequent turns.
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

async fn run_hand_task(
    store: Arc<Store>,
    llm: Arc<OpenAICompatClient>,
    hand_cfg: HandConfig,
    snapshot: Arc<SnapshotManager>,
    workspace: Workspace,
    task_id: String,
    head_id: String,
    hand_id: String,
    scope: String,
    notify_scope: Option<String>,
    reply_to: Option<Uuid>,
    prompt: String,
    input: String,
    batch_calls: Option<Vec<BatchCall>>,
    autist: AutistMode,
    filter: crate::runtime::FilterMode,
    poverty: crate::runtime::PovertyMode,
    ems: Option<EmsHandle>,
    cancel: CancellationToken,
) {
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
    let tools = snap.hand_tools.clone();
    let plugins = snap.plugins.clone();

    let bundle_builder = HandBundleBuilder::new_with_snapshot(
        store.clone(),
        workspace.root().to_path_buf(),
        snapshot.clone(),
    );
    let bundle_cfg = HandBundleConfig::new(&task_id, &head_id, &prompt, &input)
        .with_autist(autist)
        .with_filter(filter)
        .with_poverty(poverty);
    let mut messages = bundle_builder.build(&bundle_cfg);

    let tool_choice = serde_json::json!("auto");
    let policy = RetryPolicy::default_llm();
    let cwd: SharedCwd = Arc::new(Mutex::new(workspace.root().to_path_buf()));
    let dispatch_cwd = workspace.root().to_path_buf();

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
            let out = dispatch_tool(&call.name, &args_str, &format!("hand/{}", hand_id), &dispatch_cwd).await;
            let duration_ms = start.elapsed().as_millis() as u64;
            let success = tool_result_ok(&out);

            let _ = store.log_hand_exec(
                &task_id, &hand_id, 0, &call.name, &args_str, &out, success, duration_ms, "batch",
            );

            if !success {
                // Fail fast: return error to head so it can resubmit
                complete(
                    &task_id, &hand_id, &scope, notify_scope.as_deref(), reply_to,
                    false,
                    format!("FAILED: batch call {} ({}) failed: {}", i, call.name, out),
                    dispatch_cwd,
                )
                .await;
                return;
            }

            fabricated_tool_calls.push(ToolCall {
                id: call_id.clone(),
                call_type: "function".to_string(),
                function: ToolCallFunction {
                    name: call.name.clone(),
                    arguments: args_str,
                },
            });
            tool_results.push((call_id, out));
        }

        // Build pre-hydrated messages:
        // 1. Keep the system message from the bundle (messages[0])
        // 2. Fabricated assistant turn with all tool calls
        // 3. One tool_result per call
        // 4. User message with the synthesis prompt
        let system_msg = messages.remove(0);
        messages.clear();
        messages.push(system_msg);
        messages.push(ChatMessage::assistant_tool_calls(fabricated_tool_calls));
        for (call_id, result) in tool_results {
            messages.push(ChatMessage::tool_result(call_id, result));
        }
        messages.push(ChatMessage::new(Role::User, prompt.clone()));
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
            messages.clone(),
            tools.clone(),
            tool_choice.clone(),
            policy.clone(),
            |attempt, note| {
                let _ = store.log_hand_exec(
                    &task_id,
                    &hand_id,
                    iter,
                    "_llm_retry",
                    "",
                    &format!("llm retry {}: {}", attempt + 1, note),
                    true,
                    0,
                    "",
                );
            },
            Some(cancel.clone()),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let _ = store.log_hand_exec(
                    &task_id,
                    &hand_id,
                    iter,
                    "_llm_error",
                    "",
                    &format!("llm failed after retries: {}", e.message),
                    false,
                    0,
                    "",
                );
                complete(
                    &task_id,
                    &hand_id,
                    &scope,
                    notify_scope.as_deref(),
                    reply_to,
                    false,
                    format!("FAILED: llm failed after retries: {}", e.message),
                    dispatch_cwd.clone(),
                )
                .await;
                return;
            }
        };

        let _ = store.log_llm_interaction(
            "hand",
            &task_id,
            iter,
            &res.request_json,
            &res.response_json,
        );

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

        // Strict: one tool call per turn. If multiple, execute the first and
        // record that the model violated the contract.
        if res.tool_calls.len() > 1 {
            let _ = store.log_hand_exec(
                &task_id,
                &hand_id,
                iter,
                "_warning",
                "",
                "warning: model returned multiple tool calls; only the first was executed",
                true,
                0,
                "",
            );
        }

        let tc = &res.tool_calls[0];
        messages.push(ChatMessage::assistant_tool_calls(vec![tc.clone()]));

        let start = std::time::Instant::now();
        let out = if plugins.is_enabled_tool_name(&tc.function.name) {
            plugins
                .exec_hand_tool(
                    &workspace,
                    &cwd,
                    &tc.function.name,
                    &tc.function.arguments,
                    Some(cancel.clone()),
                )
                .await
        } else {
            dispatch_tool(
                &tc.function.name,
                &tc.function.arguments,
                &format!("hand/{}", hand_id),
                &dispatch_cwd,
            )
            .await
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        let success = tool_result_ok(&out);

        if success {
            tool_failure_streak = 0;
        } else {
            tool_failure_streak += 1;
        }
        let _ = store.log_hand_exec(
            &task_id,
            &hand_id,
            iter,
            &tc.function.name,
            &tc.function.arguments,
            &out,
            success,
            duration_ms,
            "",
        );

        messages.push(ChatMessage::tool_result(tc.id.clone(), out));

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
