// HandService executes tasks assigned by heads.
//
// Hard cutover: hands use provider tool calls (no fenced parsing).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::agent_tools::{Workspace, SharedCwd, exec_hand_tool, hand_tool_specs};
use crate::runtime::PluginManager;
use crate::bus::{MessageData, MessageOp, Origin, Scope, TaskMsg, respond};
use crate::history::Store;
use crate::llm::{ChatMessage, OpenAICompatClient};

use super::{HandConfig, RuntimeBus, HandBundleBuilder, HandBundleConfig};
use super::llm_harness::{chat_with_tools_retry, RetryPolicy};

const MAX_CONCURRENT_TASKS: usize = 8;

fn summarize_tool_args(tool: &str, args_json: &str) -> serde_json::Value {
    use serde_json::{json, Value};

    fn type_name(v: &Value) -> &'static str {
        match v {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }

    let Ok(v) = serde_json::from_str::<Value>(args_json) else {
        return json!({"keys": [], "raw_len": args_json.len(), "parse": "error"});
    };

    let Value::Object(map) = v else {
        return json!({"keys": [], "raw_type": type_name(&v)});
    };

    let mut out = serde_json::Map::new();
    let keys: Vec<String> = map.keys().cloned().collect();
    out.insert("keys".to_string(), json!(keys));

    // Tool-specific, conservative summaries.
    if tool == "bash" {
        if let Some(Value::String(cmd)) = map.get("command") {
            let looks_sensitive = {
                let lower = cmd.to_ascii_lowercase();
                lower.contains("api_key")
                    || lower.contains("token")
                    || lower.contains("password")
                    || lower.contains("authorization:")
                    || lower.contains("cookie:")
            };
            if looks_sensitive {
                out.insert("command_preview".to_string(), json!("<redacted>"));
                out.insert("redacted".to_string(), json!(true));
            } else {
                let preview: String = cmd.chars().take(120).collect();
                out.insert("command_preview".to_string(), json!(preview));
                out.insert("redacted".to_string(), json!(false));
            }
            out.insert("command_len".to_string(), json!(cmd.len()));
        }
        return Value::Object(out);
    }

    for (k, val) in map {
        let k_lower = k.to_ascii_lowercase();
        let is_sensitive_key = k_lower.contains("key")
            || k_lower.contains("token")
            || k_lower.contains("secret")
            || k_lower.contains("password")
            || k_lower.contains("authorization")
            || k_lower.contains("cookie");

        if is_sensitive_key {
            match val {
                Value::String(s) => out.insert(format!("{}_len", k), json!(s.len())),
                _ => out.insert(format!("{}_type", k), json!(type_name(&val))),
            };
            continue;
        }

        if matches!(k.as_str(), "content" | "patch" | "patchText") {
            if let Value::String(s) = val {
                out.insert(format!("{}_len", k), json!(s.len()));
            } else {
                out.insert(format!("{}_type", k), json!(type_name(&val)));
            }
            continue;
        }

        match val {
            Value::Bool(b) => {
                out.insert(k, json!(b));
            }
            Value::Number(n) => {
                out.insert(k, json!(n));
            }
            Value::String(s) => {
                let trimmed = s.trim();
                if trimmed.len() <= 160 {
                    out.insert(k, json!(trimmed));
                } else {
                    out.insert(format!("{}_len", k), json!(s.len()));
                }
            }
            Value::Array(a) => {
                out.insert(format!("{}_len", k), json!(a.len()));
            }
            Value::Object(o) => {
                out.insert(format!("{}_keys", k), json!(o.keys().cloned().collect::<Vec<_>>()));
            }
            Value::Null => {
                out.insert(k, Value::Null);
            }
        }
    }

    Value::Object(out)
}

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
    plugins: PluginManager,
    task_semaphore: Arc<Semaphore>,
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
        let plugins = PluginManager::load_for_workspace_root(&workspace_root);

        Self {
            bus,
            store,
            state: Arc::new(Mutex::new(HashMap::new())),
            hand_cfg,
            llm,
            workspace_root,
            plugins,
            task_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS)),
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
        let plugins = self.plugins.clone();
        let semaphore = self.task_semaphore.clone();

        tokio::spawn(async move {
            // Acquire permit before running task (limits concurrent tasks)
            let _permit = semaphore.acquire().await.expect("semaphore closed");
            run_hand_task(bus, store, llm, hand_cfg, plugins, workspace, scope, task_id, head_id, hand_id, goal, input)
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
    plugins: PluginManager,
    workspace: Workspace,
    scope: Scope,
    task_id: String,
    head_id: String,
    hand_id: String,
    goal: String,
    input: String,
) {
    let mut tools = hand_tool_specs();
    tools.extend(plugins.hand_tool_specs());
    let playbooks = plugins.hand_playbooks_md();

    let bundle_builder = HandBundleBuilder::new_with_tools_and_playbooks(
        store.clone(),
        workspace.root().to_path_buf(),
        tools.clone(),
        playbooks,
    );
    let bundle_cfg = HandBundleConfig::new(&task_id, &head_id, &goal, &input);
    let mut messages = bundle_builder.build(&bundle_cfg);

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
            plugins.exec_hand_tool(&workspace, &cwd, &tc.function.name, &tc.function.arguments).await
        } else {
            exec_hand_tool(&workspace, &cwd, store.as_ref(), &tc.function.name, &tc.function.arguments).await
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
