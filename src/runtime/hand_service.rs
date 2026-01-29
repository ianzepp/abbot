use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::bus::{MessageData, MessageOp, Scope, TaskMsg, respond};
use crate::history::Store;
use crate::tools::{Dispatcher, ExecutionContext, SharedCwd};

use super::RuntimeBus;

pub struct HandService {
    bus: RuntimeBus,
    store: Arc<Store>,
    dispatcher: Arc<Dispatcher>,
    state: Arc<Mutex<HashMap<String, TaskState>>>, // task_id -> state
}

#[derive(Clone)]
struct TaskState {
    head_id: String,
    input: String,
    assigned_hand_id: Option<String>,
    started: bool,
}

#[derive(serde::Deserialize)]
struct HandScript {
    steps: Vec<HandStep>,
}

#[derive(serde::Deserialize)]
struct HandStep {
    tool: String,
    args: String,
}

impl HandService {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, dispatcher: Dispatcher) -> Self {
        Self {
            bus,
            store,
            dispatcher: Arc::new(dispatcher),
            state: Arc::new(Mutex::new(HashMap::new())),
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
            let Scope::Task(_) = &msg.scope else {
                continue;
            };

            match msg.data.clone() {
                MessageData::Task(TaskMsg::Request { task_id, head_id, goal: _, input }) => {
                    self.on_request(msg.scope.clone(), task_id, head_id, input).await;
                }
                MessageData::Task(TaskMsg::Assigned { task_id, head_id, hand_id }) => {
                    self.on_assigned(msg.scope.clone(), task_id, head_id, hand_id).await;
                }
                _ => {}
            }
        }
    }

    async fn on_request(&self, _scope: Scope, task_id: String, head_id: String, input: String) {
        {
            let mut state = self.state.lock().unwrap();
            state
                .entry(task_id.clone())
                .or_insert(TaskState {
                    head_id: head_id.clone(),
                    input,
                    assigned_hand_id: None,
                    started: false,
                });
        }
    }

    async fn on_assigned(&self, scope: Scope, task_id: String, head_id: String, hand_id: String) {
        let input = self.input_for_task(&scope, &task_id).await.unwrap_or_else(|| String::new());
        if input.is_empty() {
            let msg = respond::task_result(
                "hand",
                scope,
                task_id,
                hand_id,
                false,
                "FAILED: task request not found for this task_id (no input to execute).".to_string(),
            );
            self.bus.publish(msg).await;
            return;
        }

        // Only run once per task.
        {
            let mut state = self.state.lock().unwrap();
            let entry = state.entry(task_id.clone()).or_insert(TaskState {
                head_id: head_id.clone(),
                input: input.clone(),
                assigned_hand_id: None,
                started: false,
            });

            entry.assigned_hand_id = Some(hand_id.clone());
            if entry.started {
                return;
            }
            entry.started = true;
        }

        // Spawn so we don't block the subscription loop.
        let bus = self.bus.clone();
        let store = self.store.clone();
        let dispatcher = self.dispatcher.clone();

        tokio::spawn(async move {
            run_hand_task(bus, store, dispatcher, scope, task_id, head_id, hand_id, input).await;
        });
    }

    async fn input_for_task(&self, scope: &Scope, task_id: &str) -> Option<String> {
        for _ in 0..5 {
            if let Some(input) = {
                let state = self.state.lock().unwrap();
                state.get(task_id).map(|s| s.input.clone())
            } {
                if !input.is_empty() {
                    return Some(input);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Fallback to DB lookup (race-safe) since requests are persisted.
        let scope_str = scope.to_string();
        match self.store.recent(&scope_str, 200) {
            Ok(messages) => messages.into_iter().rev().find_map(|m| match m.data {
                MessageData::Task(TaskMsg::Request { input, .. }) => Some(input),
                _ => None,
            }),
            Err(_) => None,
        }
    }
}

fn parse_script(input: &str) -> Result<HandScript, String> {
    serde_yaml::from_str::<HandScript>(input).map_err(|e| e.to_string())
}

async fn run_hand_task(
    bus: RuntimeBus,
    store: Arc<Store>,
    dispatcher: Arc<Dispatcher>,
    scope: Scope,
    task_id: String,
    _head_id: String,
    hand_id: String,
    input: String,
) {
    let script = match parse_script(&input) {
        Ok(s) => s,
        Err(e) => {
            let msg = respond::task_result(
                "hand",
                scope,
                task_id,
                hand_id,
                false,
                format!("FAILED: could not parse hand script: {e}. HEAD MUST PROVIDE: valid YAML with `steps:`."),
            );
            bus.publish(msg).await;
            return;
        }
    };

    if script.steps.is_empty() {
        let msg = respond::task_result(
            "hand",
            scope,
            task_id,
            hand_id,
            false,
            "FAILED: script has no steps.".to_string(),
        );
        bus.publish(msg).await;
        return;
    }

    let cwd: SharedCwd = Arc::new(Mutex::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))));
    let mut ok = true;

    let start = std::time::Instant::now();
    bus.publish(respond::task_progress(
        "hand",
        scope.clone(),
        task_id.clone(),
        hand_id.clone(),
        format!("starting {} step(s)", script.steps.len()),
    ))
    .await;

    for (i, step) in script.steps.iter().enumerate() {
        let step_no = i + 1;

        bus.publish(respond::task_progress(
            "hand",
            scope.clone(),
            task_id.clone(),
            hand_id.clone(),
            format!("step {}/{}: {}", step_no, script.steps.len(), step.tool),
        ))
        .await;

        let ctx = ExecutionContext {
            cwd: cwd.clone(),
            sender: hand_id.clone(),
            scope: scope.clone(),
        };

        let step_start = std::time::Instant::now();
        let output = match dispatcher.execute(&step.tool, &step.args, &ctx).await {
            Some(out) => out,
            None => {
                format!("error: unknown tool '{}'", step.tool)
            }
        };
        let duration_ms = step_start.elapsed().as_millis() as u64;

        let step_ok = !output.starts_with("error:");
        if let Err(e) = store.log_task_tool_call(
            &task_id,
            &hand_id,
            i,
            &step.tool,
            &step.args,
            &output,
            step_ok,
            duration_ms,
        ) {
            tracing::warn!(error = %e, "failed to log task tool call");
        }

        if !step_ok {
            ok = false;
            break;
        }
    }

    tokio::time::sleep(Duration::from_millis(10)).await;
    let elapsed_ms = start.elapsed().as_millis();

    let summary = if ok {
        format!("OK: completed {} step(s) in {}ms.", script.steps.len(), elapsed_ms)
    } else {
        format!(
            "FAILED: task did not complete. See persisted task tool calls for task_id='{}'.",
            task_id
        )
    };

    bus.publish(respond::task_result(
        "hand",
        scope,
        task_id,
        hand_id,
        ok,
        summary,
    ))
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Hub;
    use crate::history::Store;
    use crate::runtime::{HandAllocator, HeadService, RuntimeBus};
    use crate::tools::{BashTool, CdTool, ReadTool, WriteTool, EditTool, FindTool, DiffTool, PatchTool};
    use tokio::sync::RwLock;

    fn dispatcher() -> Dispatcher {
        let mut d = Dispatcher::new();
        d.register(Box::new(BashTool));
        d.register(Box::new(CdTool));
        d.register(Box::new(ReadTool));
        d.register(Box::new(WriteTool));
        d.register(Box::new(EditTool));
        d.register(Box::new(FindTool));
        d.register(Box::new(DiffTool));
        d.register(Box::new(PatchTool));
        d
    }

    #[tokio::test]
    async fn hand_runs_script_and_emits_result() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let hub = Arc::new(RwLock::new(Hub::new()));
        let bus = RuntimeBus::new(hub.clone(), store.clone());

        let scope = Scope::Task("task/test-hand-1".to_string());
        bus.create_scope(scope.clone()).await;
        bus.create_scope(Scope::from("#general")).await;

        Arc::new(HandAllocator::new(bus.clone())).start();
        Arc::new(HandService::new(bus.clone(), store.clone(), dispatcher())).start();
        Arc::new(HeadService::new(bus.clone(), "Monk", Scope::from("#general"))).start();

        // Wait for services to start.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut rx = bus.hub().read().await.subscribe(&scope).unwrap();
        let mut general = bus.hub().read().await.subscribe(&Scope::from("#general")).unwrap();

        let input = r#"
steps:
  - tool: bash
    args: "echo hello"
"#;
        let req = respond::task_request("head", scope.clone(), "test-hand-1", "Monk", "run script", input);
        bus.publish(req).await;

        let res = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = rx.recv().await.unwrap();
                if msg.op != MessageOp::Task {
                    continue;
                }
                if let MessageData::Task(TaskMsg::Result { ok, .. }) = msg.data {
                    return ok;
                }
            }
        })
        .await
        .expect("timed out waiting for result");

        assert!(res, "expected ok result");

        let saw_summary = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = general.recv().await.unwrap();
                if msg.op == MessageOp::Chat && msg.sender == "Monk" {
                    return msg.text().unwrap_or("").contains("[task test-hand-1]");
                }
            }
        })
        .await
        .expect("timed out waiting for head summary");
        assert!(saw_summary);
    }
}
