// HandService executes tasks assigned by heads.
//
// Hard cutover: hands use provider tool calls (no fenced parsing).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::agent_tools::{Workspace, SharedCwd, exec_hand_tool, hand_tool_specs};
use crate::bus::{MessageData, MessageOp, Origin, Scope, TaskMsg, respond};
use crate::history::Store;
use crate::llm::{ChatMessage, OpenAICompatClient, Role};

use super::{HandConfig, RuntimeBus};
use super::llm_harness::{chat_with_tools_retry, RetryPolicy};

pub struct HandService {
    bus: RuntimeBus,
    store: Arc<Store>,
    state: Arc<Mutex<HashMap<String, TaskState>>>,
    hand_cfg: HandConfig,
    llm: Option<Arc<OpenAICompatClient>>,
    workspace_root: PathBuf,
}

#[derive(Clone)]
struct TaskState {
    goal: String,
    input: String,
    assigned_hand_id: Option<String>,
    started: bool,
}

impl HandService {
    pub fn new(bus: RuntimeBus, store: Arc<Store>) -> Self {
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
        }
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

            if msg.op != MessageOp::Task {
                continue;
            }
            if !msg.scope.is_task() {
                continue;
            }

            match msg.data.clone() {
                MessageData::Task(TaskMsg::Request {
                    task_id,
                    goal,
                    input,
                    ..
                }) => {
                    self.on_request(task_id, goal, input).await;
                }
                MessageData::Task(TaskMsg::Assigned {
                    task_id, hand_id, ..
                }) => {
                    self.on_assigned(msg.scope.clone(), task_id, hand_id).await;
                }
                _ => {}
            }
        }
    }

    async fn on_request(&self, task_id: String, goal: String, input: String) {
        let mut state = self.state.lock().expect("hand state lock poisoned");
        state.entry(task_id).or_insert(TaskState {
            goal,
            input,
            assigned_hand_id: None,
            started: false,
        });
    }

    async fn on_assigned(&self, scope: Scope, task_id: String, hand_id: String) {
        let (goal, input) = {
            let mut state = self.state.lock().expect("hand state lock poisoned");
            let entry = state.entry(task_id.clone()).or_insert(TaskState {
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
            (entry.goal.clone(), entry.input.clone())
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

        tokio::spawn(async move {
            run_hand_task(bus, store, llm, hand_cfg, workspace, scope, task_id, hand_id, goal, input)
                .await;
        });
    }
}

fn build_initial_prompt(goal: &str, input: &str) -> String {
    let mut out = String::new();
    out.push_str("TASK\n");
    out.push_str("goal: ");
    out.push_str(goal.trim());
    out.push('\n');
    if !input.trim().is_empty() {
        out.push_str("input:\n");
        out.push_str(input.trim());
        out.push('\n');
    }
    out
}

async fn run_hand_task(
    bus: RuntimeBus,
    store: Arc<Store>,
    llm: Arc<OpenAICompatClient>,
    hand_cfg: HandConfig,
    workspace: Workspace,
    scope: Scope,
    task_id: String,
    hand_id: String,
    goal: String,
    input: String,
) {
    let system = format!(
        "{}\n\n{}",
        include_str!("hand_system.md"),
        include_str!("hand_grammar.md")
    );

    let mut messages = vec![
        ChatMessage::new(Role::System, system),
        ChatMessage::new(Role::User, build_initial_prompt(&goal, &input)),
    ];

    let tools = hand_tool_specs();
    let tool_choice = serde_json::json!("auto");
    let policy = RetryPolicy::default_llm();
    let cwd: SharedCwd = Arc::new(Mutex::new(workspace.root().to_path_buf()));

    let mut tool_failure_streak: usize = 0;

    for iter in 0..hand_cfg.max_iters {
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
            respond::task_progress(
                "hand",
                scope.clone(),
                task_id.clone(),
                hand_id.clone(),
                format!("tool: {}", tc.function.name),
            )
            .with_origin(Origin::Hand),
        )
        .await;

        let start = std::time::Instant::now();
        let out = exec_hand_tool(&workspace, &cwd, store.as_ref(), &tc.function.name, &tc.function.arguments).await;
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

        // small yield to avoid tight loops
        tokio::time::sleep(Duration::from_millis(5)).await;
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
