// HandService executes tasks assigned by heads.
//
// Hard cutover: hands use provider tool calls (no fenced parsing).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::agent_tools::{Workspace, SharedCwd, exec_hand_tool};
use crate::bus::{MessageData, MessageOp, Origin, Scope, TaskMsg, respond};
use crate::ems::EmsHandle;
use crate::history::Store;
use crate::llm::{ChatMessage, OpenAICompatClient};
use crate::runtime::summarize_tool_args;

use super::{HandConfig, RuntimeBus, HandBundleBuilder, HandBundleConfig, AutistMode, SnapshotManager};
use super::llm_harness::{chat_with_tools_retry, RetryPolicy};

const MAX_CONCURRENT_TASKS: usize = 8;

fn tool_result_error_code(tool_result_json: &str) -> Option<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(tool_result_json) else {
        return None;
    };
    v.get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
}

pub struct HandService {
    bus: RuntimeBus,
    store: Arc<Store>,
    state: Arc<Mutex<HashMap<String, TaskState>>>,
    hand_cfg: HandConfig,
    llm: Option<Arc<OpenAICompatClient>>,
    workspace_root: PathBuf,
    snapshot: Arc<SnapshotManager>,
    task_semaphore: Arc<Semaphore>,
    autist: AutistMode,
    ems: Option<EmsHandle>,
    cancels: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

#[derive(Clone)]
struct TaskState {
    head_id: String,
    goal: String,
    input: String,
    assigned_hand_id: Option<String>,
    started: bool,
}

impl HandService {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, snapshot: Arc<SnapshotManager>) -> Self {
        let hand_cfg = HandConfig::from_env();
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

        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            bus,
            store,
            state: Arc::new(Mutex::new(HashMap::new())),
            hand_cfg,
            llm,
            workspace_root,
            snapshot,
            task_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS)),
            autist: AutistMode::None,
            ems: None,
            cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_autist(mut self, autist: AutistMode) -> Self {
        self.autist = autist;
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

    async fn run(&self) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        tracing::info!("hand service started");

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if msg.op == MessageOp::Event && msg.origin == Origin::System {
                if let MessageData::Event { kind, .. } = &msg.data {
                    if kind == "collective_reboot" {
                        {
                            let mut state = self.state.lock().expect("hand state lock poisoned");
                            state.clear();
                        }
                        self.snapshot.refresh();
                        tracing::info!("refreshed runtime snapshot (collective reboot)");
                        continue;
                    }
                }
            }

            if msg.op != MessageOp::Task {
                continue;
            }
            if !msg.scope.is_task() {
                continue;
            }

            match msg.data.clone() {
                MessageData::Task(TaskMsg::Request {
                    task_id,
                    head_id,
                    goal,
                    input,
                    ..
                }) => {
                    self.on_request(task_id, head_id, goal, input).await;
                }
                MessageData::Task(TaskMsg::Assigned {
                    task_id, hand_id, ..
                }) => {
                    self.on_assigned(msg.scope.clone(), task_id, hand_id).await;
                }
                MessageData::Task(TaskMsg::Cancel { task_id, .. }) => {
                    let cancel = {
                        let cancels = self.cancels.lock().expect("hand cancels lock poisoned");
                        cancels.get(&task_id).cloned()
                    };
                    if let Some(c) = cancel {
                        c.cancel();
                    }
                }
                _ => {}
            }
        }
    }

    async fn on_request(&self, task_id: String, head_id: String, goal: String, input: String) {
        let mut state = self.state.lock().expect("hand state lock poisoned");
        state.entry(task_id).or_insert(TaskState {
            head_id,
            goal,
            input,
            assigned_hand_id: None,
            started: false,
        });
    }

    async fn on_assigned(&self, scope: Scope, task_id: String, hand_id: String) {
        let (head_id, goal, input) = {
            let mut state = self.state.lock().expect("hand state lock poisoned");
            let entry = state.entry(task_id.clone()).or_insert(TaskState {
                head_id: "unknown".to_string(),
                goal: "".to_string(),
                input: "".to_string(),
                assigned_hand_id: None,
                started: false,
            });
            entry.assigned_hand_id = Some(hand_id.clone());
            if entry.started {
                return;
            }
            entry.started = true;
            (entry.head_id.clone(), entry.goal.clone(), entry.input.clone())
        };

        let Some(llm) = self.llm.clone() else {
            self.bus
                .publish(
                    respond::task_result(
                        "hand",
                        scope,
                        task_id,
                        hand_id,
                        false,
                        "FAILED: hand LLM disabled (set HAND_MODEL/BASE_URL).".to_string(),
                    )
                    .with_origin(Origin::Hand),
                )
                .await;
            return;
        };

        let bus = self.bus.clone();
        let store = self.store.clone();
        let hand_cfg = self.hand_cfg.clone();
        let workspace = Workspace::new(self.workspace_root.clone());
        let snapshot = self.snapshot.clone();
        let semaphore = self.task_semaphore.clone();
        let autist = self.autist.clone();
        let ems = self.ems.clone();
        let cancels = self.cancels.clone();

        tokio::spawn(async move {
            // Acquire permit before running task (limits concurrent tasks)
            let _permit = semaphore.acquire().await.expect("semaphore closed");

            let cancel = CancellationToken::new();
            {
                let mut map = cancels.lock().expect("hand cancels lock poisoned");
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
                cancels: cancels.clone(),
            };

            run_hand_task(
                bus,
                store,
                llm,
                hand_cfg,
                snapshot,
                workspace,
                scope,
                task_id,
                head_id,
                hand_id,
                goal,
                input,
                autist,
                ems,
                cancel,
            )
            .await;
            // Permit automatically released when _permit drops
        });
    }
}

async fn run_hand_task(
    bus: RuntimeBus,
    store: Arc<Store>,
    llm: Arc<OpenAICompatClient>,
    hand_cfg: HandConfig,
    snapshot: Arc<SnapshotManager>,
    workspace: Workspace,
    scope: Scope,
    task_id: String,
    head_id: String,
    hand_id: String,
    goal: String,
    input: String,
    autist: AutistMode,
    ems: Option<EmsHandle>,
    cancel: CancellationToken,
) {
    let snap = snapshot.get();
    let tools = snap.hand_tools.clone();
    let plugins = snap.plugins.clone();

    let bundle_builder = HandBundleBuilder::new_with_snapshot(
        store.clone(),
        workspace.root().to_path_buf(),
        snapshot.clone(),
    );
    let bundle_cfg = HandBundleConfig::new(&task_id, &head_id, &goal, &input)
        .with_autist(autist);
    let mut messages = bundle_builder.build(&bundle_cfg);

    let tool_choice = serde_json::json!("auto");
    let policy = RetryPolicy::default_llm();
    let cwd: SharedCwd = Arc::new(Mutex::new(workspace.root().to_path_buf()));

    let mut tool_failure_streak: usize = 0;

    for iter in 0..hand_cfg.max_iters {
        if cancel.is_cancelled() {
            bus.publish(
                respond::task_result(
                    "hand",
                    scope,
                    task_id,
                    hand_id,
                    false,
                    "FAILED: task cancelled".to_string(),
                )
                .with_origin(Origin::Hand),
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
                bus.publish(
                    respond::task_result(
                        "hand",
                        scope,
                        task_id,
                        hand_id,
                        false,
                        format!("FAILED: llm failed after retries: {}", e.message),
                    )
                    .with_origin(Origin::Hand),
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
            bus.publish(
                respond::task_result(
                    "hand",
                    scope,
                    task_id,
                    hand_id,
                    ok,
                    if ok {
                        content.trim().to_string()
                    } else {
                        "FAILED: model produced no tool calls and no final content".to_string()
                    },
                )
                .with_origin(Origin::Hand),
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

        bus.publish(
            respond::task_tool_call(
                "hand",
                scope.clone(),
                task_id.clone(),
                hand_id.clone(),
                tc.id.clone(),
                tc.function.name.clone(),
                summarize_tool_args(&tc.function.name, &tc.function.arguments),
            )
            .with_origin(Origin::Hand),
        )
        .await;

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
            exec_hand_tool(
                &workspace,
                &cwd,
                store.as_ref(),
                ems.as_ref(),
                &tc.function.name,
                &tc.function.arguments,
                Some(cancel.clone()),
            )
            .await
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        let success = tool_result_ok(&out);

        bus.publish(
            respond::task_tool_done(
                "hand",
                scope.clone(),
                task_id.clone(),
                hand_id.clone(),
                tc.id.clone(),
                tc.function.name.clone(),
                success,
                duration_ms,
                if success {
                    None
                } else {
                    tool_result_error_code(&out)
                },
            )
            .with_origin(Origin::Hand),
        )
        .await;

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
            bus.publish(
                respond::task_result(
                    "hand",
                    scope,
                    task_id,
                    hand_id,
                    false,
                    "FAILED: 5 consecutive tool failures".to_string(),
                )
                .with_origin(Origin::Hand),
            )
            .await;
            return;
        }
    }

    bus.publish(
        respond::task_result(
            "hand",
            scope,
            task_id,
            hand_id,
            false,
            "FAILED: iteration limit reached without final content".to_string(),
        )
        .with_origin(Origin::Hand),
    )
    .await;
}

fn tool_result_ok(tool_result_json: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(tool_result_json) {
        Ok(v) => v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false),
        Err(_) => false,
    }
}
