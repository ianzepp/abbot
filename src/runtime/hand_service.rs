// HandService executes tasks assigned by heads.
//
// Hard cutover: hands use provider tool calls (no fenced parsing).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::agent_tools::{SharedCwd, Workspace, exec_hand_tool};
use crate::kernel::{Frame, FrameOp};
use crate::ems::EmsHandle;
use crate::history::Store;
use crate::llm::{ChatMessage, OpenAICompatClient};
use crate::runtime::Kernel;

use super::llm_harness::{RetryPolicy, chat_with_tools_retry};
use super::{
    AutistMode, HandBundleBuilder, HandBundleConfig, HandConfig, SnapshotManager,
};

const MAX_CONCURRENT_TASKS: usize = 8;

pub struct HandService {
    store: Arc<Store>,
    hand_cfg: HandConfig,
    llm: Option<Arc<OpenAICompatClient>>,
    workspace_root: PathBuf,
    snapshot: Arc<SnapshotManager>,
    task_semaphore: Arc<Semaphore>,
    autist: AutistMode,
    ems: Option<EmsHandle>,
    cancels: Arc<Mutex<HashMap<String, CancellationToken>>>,
    hand_id: String,
}

impl HandService {
    pub fn new(
        store: Arc<Store>,
        workspace_root: PathBuf,
        snapshot: Arc<SnapshotManager>,
    ) -> Self {
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

        Self {
            store,
            hand_cfg,
            llm,
            workspace_root,
            snapshot,
            task_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS)),
            autist: AutistMode::None,
            ems: None,
            cancels: Arc::new(Mutex::new(HashMap::new())),
            hand_id: std::env::var("HAND_ID").unwrap_or_else(|_| "hand-0".to_string()),
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
        let req = Frame::req(
            "task:lease",
            serde_json::json!({"hand_id": self.hand_id}),
        )
        .with_actor(format!("hand/{}", self.hand_id));

        let mut rx = dispatcher.dispatch(
            req,
            self.workspace_root.clone(),
            CancellationToken::new(),
        );
        let frame = rx.recv().await?;
        if frame.op != FrameOp::Ok {
            return None;
        }
        let v = frame.data?;
        Some(TaskLease {
            task_id: v.get("task_id")?.as_str()?.to_string(),
            head_id: v.get("head_id").and_then(|x| x.as_str()).unwrap_or("unknown").to_string(),
            goal: v.get("goal").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            input: v.get("input").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        })
    }

    async fn run_one_task(&self, task: TaskLease, _permit: OwnedSemaphorePermit) {
        let task_id = task.task_id.clone();
        let hand_id = self.hand_id.clone();

        let Some(llm) = self.llm.clone() else {
            self.complete_task(
                &task_id,
                false,
                "FAILED: hand LLM disabled (set HAND_MODEL/BASE_URL).".to_string(),
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
            task.goal,
            task.input,
            self.autist.clone(),
            self.ems.clone(),
            cancel,
        )
        .await;
    }

    async fn complete_task(&self, task_id: &str, ok: bool, summary: String) {
        let Some(k) = Kernel::get() else {
            return;
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "task:complete",
            serde_json::json!({"task_id": task_id, "ok": ok, "summary": summary}),
        )
        .with_actor(format!("hand/{}", self.hand_id));
        let mut rx = dispatcher.dispatch(
            req,
            self.workspace_root.clone(),
            CancellationToken::new(),
        );
        let _ = rx.recv().await;
    }
}

#[derive(Debug, Clone)]
struct TaskLease {
    task_id: String,
    head_id: String,
    goal: String,
    input: String,
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
    goal: String,
    input: String,
    autist: AutistMode,
    ems: Option<EmsHandle>,
    cancel: CancellationToken,
) {
    async fn complete(task_id: &str, hand_id: &str, ok: bool, summary: String) {
        let Some(k) = Kernel::get() else {
            return;
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "task:complete",
            serde_json::json!({"task_id": task_id, "ok": ok, "summary": summary}),
        )
        .with_actor(format!("hand/{hand_id}"));
        let mut rx = dispatcher.dispatch(
            req,
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            CancellationToken::new(),
        );
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
    let bundle_cfg = HandBundleConfig::new(&task_id, &head_id, &goal, &input).with_autist(autist);
    let mut messages = bundle_builder.build(&bundle_cfg);

    let tool_choice = serde_json::json!("auto");
    let policy = RetryPolicy::default_llm();
    let cwd: SharedCwd = Arc::new(Mutex::new(workspace.root().to_path_buf()));

    let mut tool_failure_streak: usize = 0;

    for iter in 0..hand_cfg.max_iters {
        if cancel.is_cancelled() {
            complete(&task_id, &hand_id, false, "FAILED: task cancelled".to_string()).await;
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
                    false,
                    format!("FAILED: llm failed after retries: {}", e.message),
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
                ok,
                if ok {
                    content.trim().to_string()
                } else {
                    "FAILED: model produced no tool calls and no final content".to_string()
                },
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
            exec_hand_tool(
                &workspace,
                &cwd,
                store.as_ref(),
                ems.as_ref(),
                &format!("hand/{}", hand_id),
                &tc.function.name,
                &tc.function.arguments,
                Some(cancel.clone()),
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
                false,
                "FAILED: 5 consecutive tool failures".to_string(),
            )
            .await;
            return;
        }
    }

    complete(
        &task_id,
        &hand_id,
        false,
        "FAILED: iteration limit reached without final content".to_string(),
    )
    .await;
}

fn tool_result_ok(tool_result_json: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(tool_result_json) {
        Ok(v) => v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false),
        Err(_) => false,
    }
}
