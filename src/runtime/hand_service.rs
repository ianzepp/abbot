use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::bus::{MessageData, MessageOp, Origin, Scope, TaskMsg, respond};
use crate::history::Store;
use crate::tools::{Dispatcher, ExecutionContext, SharedCwd};

use super::RuntimeBus;
use super::hand_parser::{EchoMode, ExecAction, parse_hand_response};

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
            )
            .with_origin(Origin::Hand);
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

fn echo_excerpt(output: &str, action: &ExecAction) -> Option<String> {
    const MAX_LINES: usize = 80;
    const MAX_CHARS: usize = 4000;

    match action.echo {
        EchoMode::None => None,
        EchoMode::Summary => {
            let line_count = output.lines().count();
            let char_count = output.chars().count();
            Some(format!("(summary) lines={} chars={}", line_count, char_count))
        }
        EchoMode::Full => {
            let clipped = clip_lines_chars(output, MAX_LINES, MAX_CHARS);
            if clipped.trim().is_empty() { None } else { Some(clipped) }
        }
        EchoMode::Head => {
            let n = action.head.unwrap_or(20).min(MAX_LINES);
            let clipped = clip_lines_chars(&head_lines(output, n), MAX_LINES, MAX_CHARS);
            if clipped.trim().is_empty() { None } else { Some(clipped) }
        }
        EchoMode::Tail => {
            let n = action.tail.unwrap_or(20).min(MAX_LINES);
            let clipped = clip_lines_chars(&tail_lines(output, n), MAX_LINES, MAX_CHARS);
            if clipped.trim().is_empty() { None } else { Some(clipped) }
        }
    }
}

fn head_lines(s: &str, n: usize) -> String {
    s.lines().take(n).collect::<Vec<_>>().join("\n")
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

fn clip_lines_chars(s: &str, max_lines: usize, max_chars: usize) -> String {
    let mut out = s
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n");

    if out.chars().count() > max_chars {
        out = out.chars().take(max_chars).collect::<String>();
    }
    out
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
    if let Ok(script) = parse_script(&input) {
        if script.steps.is_empty() {
            let msg = respond::task_result(
                "hand",
                scope,
                task_id,
                hand_id,
                false,
                "FAILED: script has no steps.".to_string(),
            )
            .with_origin(Origin::Hand);
            bus.publish(msg).await;
            return;
        }

        run_script(bus, store, dispatcher, scope, task_id, hand_id, script).await;
        return;
    }

    // Fallback: interpret the input as a pre-rendered hand response (used for testing / transitional mode).
    let parsed = parse_hand_response(&input);
    if parsed.execs.is_empty() && parsed.result.is_none() {
        let msg = respond::task_result(
            "hand",
            scope,
            task_id,
            hand_id,
            false,
            "FAILED: could not parse task input as YAML `steps:` or as a hand response.\nHEAD MUST PROVIDE: valid YAML `steps:` (legacy) OR configure hand LLM (preferred).".to_string(),
        )
        .with_origin(Origin::Hand);
        bus.publish(msg).await;
        return;
    }

    run_parsed_hand_response(bus, store, dispatcher, scope, task_id, hand_id, parsed.execs, parsed.result).await;
}

async fn run_script(
    bus: RuntimeBus,
    store: Arc<Store>,
    dispatcher: Arc<Dispatcher>,
    scope: Scope,
    task_id: String,
    hand_id: String,
    script: HandScript,
) {
    let cwd: SharedCwd = Arc::new(Mutex::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))));
    let start = std::time::Instant::now();

    bus.publish(
        respond::task_progress(
            "hand",
            scope.clone(),
            task_id.clone(),
            hand_id.clone(),
            format!("starting {} step(s)", script.steps.len()),
        )
        .with_origin(Origin::Hand),
    )
    .await;

    let mut ok = true;

    for (i, step) in script.steps.iter().enumerate() {
        let step_no = i + 1;

        bus.publish(
            respond::task_progress(
                "hand",
                scope.clone(),
                task_id.clone(),
                hand_id.clone(),
                format!("step {}/{}: {}", step_no, script.steps.len(), step.tool),
            )
            .with_origin(Origin::Hand),
        )
        .await;

        let ctx = ExecutionContext {
            cwd: cwd.clone(),
            sender: hand_id.clone(),
            scope: scope.clone(),
        };

        let step_start = std::time::Instant::now();
        let output = match dispatcher.execute(&step.tool, &step.args, &ctx).await {
            Some(out) => out,
            None => format!("error: unknown tool '{}'", step.tool),
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

    bus.publish(
        respond::task_result("hand", scope, task_id, hand_id, ok, summary).with_origin(Origin::Hand),
    )
    .await;
}

async fn run_parsed_hand_response(
    bus: RuntimeBus,
    store: Arc<Store>,
    dispatcher: Arc<Dispatcher>,
    scope: Scope,
    task_id: String,
    hand_id: String,
    execs: Vec<ExecAction>,
    result: Option<super::hand_parser::ResultAction>,
) {
    let cwd: SharedCwd = Arc::new(Mutex::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))));
    let start = std::time::Instant::now();

    bus.publish(
        respond::task_progress(
            "hand",
            scope.clone(),
            task_id.clone(),
            hand_id.clone(),
            format!("starting {} exec(s)", execs.len()),
        )
        .with_origin(Origin::Hand),
    )
    .await;

    let mut ok = true;
    for (i, action) in execs.iter().enumerate() {
        bus.publish(
            respond::task_progress(
                "hand",
                scope.clone(),
                task_id.clone(),
                hand_id.clone(),
                format!("exec {}/{}: {}", i + 1, execs.len(), action.tool),
            )
            .with_origin(Origin::Hand),
        )
        .await;

        let ctx = ExecutionContext {
            cwd: cwd.clone(),
            sender: hand_id.clone(),
            scope: scope.clone(),
        };

        let step_start = std::time::Instant::now();
        let output = match dispatcher.execute(&action.tool, &action.content, &ctx).await {
            Some(out) => out,
            None => format!("error: unknown tool '{}'", action.tool),
        };
        let duration_ms = step_start.elapsed().as_millis() as u64;

        let step_ok = !output.starts_with("error:");
        if let Err(e) = store.log_task_tool_call(
            &task_id,
            &hand_id,
            i,
            &action.tool,
            &action.content,
            &output,
            step_ok,
            duration_ms,
        ) {
            tracing::warn!(error = %e, "failed to log task tool call");
        }

        if let Some(excerpt) = echo_excerpt(&output, action) {
            bus.publish(
                respond::task_echo(
                    "hand",
                    scope.clone(),
                    task_id.clone(),
                    hand_id.clone(),
                    action.tool.clone(),
                    excerpt,
                )
                .with_origin(Origin::Hand),
            )
            .await;
        }

        if !step_ok {
            ok = false;
            break;
        }
    }

    tokio::time::sleep(Duration::from_millis(10)).await;
    let elapsed_ms = start.elapsed().as_millis();

    let (final_ok, summary) = match result {
        Some(r) => (r.ok && ok, r.text.trim().to_string()),
        None => (
            ok,
            if ok {
                format!("OK: completed {} exec(s) in {}ms.", execs.len(), elapsed_ms)
            } else {
                format!("FAILED: task did not complete. See persisted task tool calls for task_id='{}'.", task_id)
            },
        ),
    };

    bus.publish(
        respond::task_result("hand", scope, task_id, hand_id, final_ok, summary).with_origin(Origin::Hand),
    )
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
        let req = respond::task_request("head", scope.clone(), "test-hand-1", "Monk", "run script", input)
            .with_origin(Origin::Head);
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
                if msg.op == MessageOp::Chat && msg.origin == Origin::Head {
                    return msg.text().unwrap_or("").contains("[task test-hand-1]");
                }
            }
        })
        .await
        .expect("timed out waiting for head summary");
        assert!(saw_summary);
    }

    #[tokio::test]
    async fn harness_emits_echo_excerpt_when_requested() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let hub = Arc::new(RwLock::new(Hub::new()));
        let bus = RuntimeBus::new(hub.clone(), store.clone());

        let scope = Scope::Task("task/test-hand-echo-1".to_string());
        bus.create_scope(scope.clone()).await;
        bus.create_scope(Scope::from("#general")).await;

        Arc::new(HandAllocator::new(bus.clone())).start();
        Arc::new(HandService::new(bus.clone(), store.clone(), dispatcher())).start();
        Arc::new(HeadService::new(bus.clone(), "Monk", Scope::from("#general"))).start();

        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut rx = bus.hub().read().await.subscribe(&scope).unwrap();

        let input = r#"
<exec tool="bash" reason="emit" echo="head" head="1">printf 'a\nb\n'</exec>
<result ok="true">ok</result>
"#;
        let req = respond::task_request("head", scope.clone(), "test-hand-echo-1", "Monk", "run exec", input)
            .with_origin(Origin::Head);
        bus.publish(req).await;

        let (mut saw_echo, mut saw_result_ok) = (false, false);
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = rx.recv().await.unwrap();
                if msg.op != MessageOp::Task {
                    continue;
                }
                match msg.data {
                    MessageData::Task(TaskMsg::Echo { content, .. }) => {
                        if content.contains('a') && !content.contains('b') {
                            saw_echo = true;
                        }
                    }
                    MessageData::Task(TaskMsg::Result { ok, .. }) => {
                        saw_result_ok = ok;
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("timed out");

        assert!(saw_echo, "expected an echo excerpt");
        assert!(saw_result_ok, "expected ok result");
    }
}
