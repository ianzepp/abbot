// HeadService is the AI decision-maker that converts needs into tasks.
//
// Heads are purely reactive - they don't watch scopes directly. Instead, they
// receive needs from NeedService (dispatched to their mailbox) and process them
// by calling an LLM that can create tasks, send chat messages, etc. When done
// processing a need, the head emits NeedMsg::Fulfilled.
//
// The head is intentionally stateless between needs - all context comes from
// the message store, enabling restart without data loss.

use std::collections::{HashMap, VecDeque};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use uuid::Uuid;

use tokio::sync::mpsc;

use crate::Scope;
use crate::agent_tools::{SharedCwd, ToolEffect, Workspace, exec_head_tool, head_tool_effect};
use crate::ems::EmsHandle;
use crate::history::Store;
use crate::llm::{OpenAICompatClient, ToolCall};
use crate::recall::Search;
use crate::runtime::AppConfig;
use crate::runtime::Kernel;
use crate::runtime::summarize_tool_args;
use serde_json::json;

use super::proc_service::ProcHandle;

use super::{
    GenerationMode, HeadBundleBuilder, HeadBundleConfig, HeadConfig, SessionWriteLocks,
    SnapshotManager, TarsDials, read_optional_file, workspace_config_from_root,
};

// Context for the need currently being processed
#[derive(Debug, Clone)]
struct ActiveNeed {
    need_id: String,
    need_text: String,
    context: String,
    scope: Option<String>,
    reply_to: Option<Uuid>,

    // Local state for multi-step execution.
    wait_kind: Option<WaitKind>,
    pending_task_ids: Vec<String>,
    // Persisted transcript for this need so external-tool resumes are true continuations.
    llm_messages: Vec<crate::llm::ChatMessage>,

    // External tool call queue (one redirect per transport).
    pending_external: Vec<ToolCall>,
    active_external_id: Option<String>,

    // Simple runaway brake: remember recent external tool call signatures.
    recent_external_sigs: VecDeque<u64>,
    wait_done_sent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitKind {
    Tasks,
    ExternalTool,
}

#[derive(Debug, Clone)]
enum ResumeMsg {
    ExternalTool {
        tool_call_id: String,
        output: String,
    },
    Need(ActiveNeed),
    TasksDone {
        need_id: String,
    },
}

pub struct HeadService {
    _proc: ProcHandle,
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>, // Scopes this head can read context from
    memory: Option<Arc<Search>>,
    llm: Option<Arc<OpenAICompatClient>>,
    workspace_root: PathBuf,
    snapshot: Arc<SnapshotManager>,
    active_need: tokio::sync::Mutex<Option<ActiveNeed>>,
    external_waiters: tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Receiver<String>>>,
    resume_tx: mpsc::Sender<ResumeMsg>,
    resume_rx: tokio::sync::Mutex<Option<mpsc::Receiver<ResumeMsg>>>,
    generation: GenerationMode,
    filter: crate::runtime::FilterMode,
    poverty: crate::runtime::PovertyMode,
    session_locks: SessionWriteLocks,
    ems: Option<EmsHandle>,
}

fn head_context_budget_tokens() -> Option<u32> {
    // Sliding window + rough uniform context sizes. Keep this simple until we
    // have per-model metadata again.
    Some(100_000)
}

fn head_time_gap_marker_minutes() -> Option<u64> {
    let app = AppConfig::global();
    let ws = app
        .workspace_path()
        .ok()
        .map(|p| crate::runtime::WorkspaceConfigToml::load_from_workspace_root(&p))
        .unwrap_or_default();

    let v = ws
        .head
        .time_gap_marker_minutes
        .or(app.head.time_gap_marker_minutes)
        .unwrap_or(60);
    if v == 0 { None } else { Some(v) }
}

fn load_tars_dials(workspace_root: &std::path::Path) -> TarsDials {
    let config_path = workspace_config_from_root(workspace_root);
    if !config_path.exists() {
        return TarsDials::default();
    }

    let config_str = match read_optional_file(&config_path) {
        Ok(Some(s)) => s,
        _ => return TarsDials::default(),
    };

    let config: toml::Table = match config_str.parse() {
        Ok(t) => t,
        Err(_) => return TarsDials::default(),
    };

    let Some(tars) = config.get("tars").and_then(|v| v.as_table()) else {
        return TarsDials::default();
    };

    TarsDials {
        humor: tars
            .get("humor")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        honesty: tars
            .get("honesty")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        sarcasm: tars
            .get("sarcasm")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        verbosity: tars
            .get("verbosity")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        confidence: tars
            .get("confidence")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        curiosity: tars
            .get("curiosity")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        patience: tars
            .get("patience")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        formality: tars
            .get("formality")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        empathy: tars
            .get("empathy")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        pedantry: tars
            .get("pedantry")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        initiative: tars
            .get("initiative")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        optimism: tars
            .get("optimism")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
        caution: tars
            .get("caution")
            .and_then(|v| v.as_float())
            .map(|f| f as f32),
    }
}

impl HeadService {
    pub fn new(
        proc: ProcHandle,
        store: Arc<Store>,
        workspace_root: PathBuf,
        head_id: impl Into<String>,
        scopes: Vec<Scope>,
        memory: Option<Arc<Search>>,
        snapshot: Arc<SnapshotManager>,
        session_locks: SessionWriteLocks,
    ) -> Self {
        let head_id = head_id.into();
        let head_cfg = HeadConfig::from_config();

        let llm = if head_cfg.llm.enabled {
            tracing::debug!(
                head = %head_id,
                base_url = %head_cfg.llm.base_url,
                model = %head_cfg.llm.model,
                api_key_set = !head_cfg.llm.api_key.is_empty(),
                temperature = ?head_cfg.llm.temperature,
                max_tokens = ?head_cfg.llm.max_tokens,
                heartbeat_tick = head_cfg.heartbeat_tick,
                "head llm config"
            );
            Some(Arc::new(OpenAICompatClient::new(
                &head_cfg.llm.base_url,
                &head_cfg.llm.api_key,
                &head_cfg.llm.model,
                head_cfg.llm.temperature,
                head_cfg.llm.max_tokens,
                head_cfg.llm.extra_headers.clone(),
            )))
        } else {
            tracing::warn!(
                head = %head_id,
                model = %head_cfg.llm.model,
                base_url = %head_cfg.llm.base_url,
                api_key_set = !head_cfg.llm.api_key.is_empty(),
                "head llm disabled (configure head.model and providers.<provider>.base_url in abbot.toml)"
            );
            None
        };

        let (resume_tx, resume_rx) = mpsc::channel::<ResumeMsg>(32);

        Self {
            _proc: proc,
            store,
            head_id,
            scopes,
            memory,
            llm,
            workspace_root,
            snapshot,
            active_need: tokio::sync::Mutex::new(None),
            external_waiters: tokio::sync::Mutex::new(HashMap::new()),
            resume_tx,
            resume_rx: tokio::sync::Mutex::new(Some(resume_rx)),
            generation: head_cfg.generation.clone(),
            filter: head_cfg.filter.clone(),
            poverty: head_cfg.poverty.clone(),
            session_locks,
            ems: None,
        }
    }

    pub fn with_generation(mut self, generation: GenerationMode) -> Self {
        self.generation = generation;
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
        let mut resume_rx = {
            let mut guard = self.resume_rx.lock().await;
            guard.take().expect("head resume receiver already taken")
        };

        tracing::debug!(head = %self.head_id, "head service started");
        loop {
            let idle = self.active_need.lock().await.is_none();
            if idle {
                let resume_tx = self.resume_tx.clone();
                let head_id = self.head_id.clone();
                let cwd = self.workspace_root.clone();
                tokio::spawn(async move {
                    let Some(k) = Kernel::get() else {
                        return;
                    };
                    let dispatcher = k.dispatcher().await;
                    let req = crate::kernel::Frame::req("need:lease", serde_json::json!({}))
                        .with_actor(format!("head/{head_id}"));
                    let mut rx =
                        dispatcher.dispatch(req, cwd, tokio_util::sync::CancellationToken::new());

                    let Some(frame) = rx.recv().await else {
                        return;
                    };
                    if frame.op != crate::kernel::FrameOp::Ok {
                        return;
                    }
                    let Some(v) = frame.data else {
                        return;
                    };

                    let need_id = v
                        .get("need_id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let need_text = v
                        .get("need")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let context = v
                        .get("context")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let scope = v
                        .get("scope")
                        .and_then(|x| x.as_str())
                        .unwrap_or("main")
                        .to_string();
                    let reply_to = v
                        .get("reply_to")
                        .and_then(|x| x.as_str())
                        .and_then(|s| uuid::Uuid::parse_str(s).ok());

                    if need_id.trim().is_empty() || need_text.trim().is_empty() {
                        return;
                    }

                    let need = ActiveNeed {
                        need_id,
                        need_text,
                        context,
                        scope: Some(scope),
                        reply_to,
                        wait_kind: None,
                        pending_task_ids: Vec::new(),
                        llm_messages: Vec::new(),
                        pending_external: Vec::new(),
                        active_external_id: None,
                        recent_external_sigs: VecDeque::new(),
                        wait_done_sent: false,
                    };

                    let _ = resume_tx.send(ResumeMsg::Need(need)).await;
                });
            }

            let resume = resume_rx.recv().await;
            let Some(resume) = resume else {
                return;
            };

            match resume {
                ResumeMsg::ExternalTool {
                    tool_call_id,
                    output,
                } => {
                    let mut dispatch_next: Option<(String, Uuid, ToolCall)> = None;
                    let mut resume_need: Option<ActiveNeed> = None;

                    {
                        let mut active = self.active_need.lock().await;
                        let Some(n) = active.as_mut() else {
                            continue;
                        };

                        if n.wait_kind != Some(WaitKind::ExternalTool) {
                            continue;
                        }

                        if n.active_external_id.as_deref() != Some(tool_call_id.as_str()) {
                            continue;
                        }

                        // Append tool output to the persisted transcript.
                        const MAX_TOOL_OUTPUT_CHARS: usize = 20_000;
                        let output = truncate(&output, MAX_TOOL_OUTPUT_CHARS);
                        n.llm_messages
                            .push(crate::llm::ChatMessage::tool_result(tool_call_id.clone(), output));

                        // Remove completed call from queue.
                        if let Some(pos) = n.pending_external.iter().position(|tc| tc.id == tool_call_id) {
                            n.pending_external.remove(pos);
                        }

                        if let Some(reply_to) = n.reply_to {
                            let scope = n.scope.clone().unwrap_or_else(|| "main".to_string());

                            if let Some(next) = n.pending_external.first().cloned() {
                                n.active_external_id = Some(next.id.clone());
                                n.wait_kind = Some(WaitKind::ExternalTool);
                                n.wait_done_sent = false;
                                dispatch_next = Some((scope, reply_to, next));
                            } else {
                                n.active_external_id = None;
                                n.wait_kind = None;
                                n.wait_done_sent = false;
                                resume_need = Some(n.clone());
                            }
                        } else {
                            n.pending_external.clear();
                            n.active_external_id = None;
                            n.wait_kind = None;
                            n.wait_done_sent = false;
                            resume_need = Some(n.clone());
                        }
                    }

                    if let Some((scope, reply_to, tc)) = dispatch_next {
                        tracing::debug!(
                            head = %self.head_id,
                            scope = %scope,
                            reply_to = ?reply_to,
                            tool = %tc.function.name,
                            "dispatching next external tool call"
                        );

                        if let Err(e) = self.dispatch_external_tool_call(&scope, reply_to, &tc).await {
                            // Treat as a synthetic tool result so the LLM can recover.
                            let mut active = self.active_need.lock().await;
                            if let Some(n) = active.as_mut() {
                                n.llm_messages.push(crate::llm::ChatMessage::tool_result(
                                    tc.id.clone(),
                                    format!("External tool dispatch failed: {e}"),
                                ));
                                n.wait_kind = None;
                                n.active_external_id = None;
                                n.pending_external.clear();
                                n.wait_done_sent = false;
                                resume_need = Some(n.clone());
                            }
                        } else {
                            self.arm_external_waiter(tc.id.clone()).await;
                        }
                    }

                    if let Some(need) = resume_need {
                        tracing::debug!(
                            head = %self.head_id,
                            need_id = %need.need_id,
                            scope = %need.scope.as_deref().unwrap_or("main"),
                            reply_to = ?need.reply_to,
                            "external tool queue completed; resuming need"
                        );
                        self.clone().process_need(need).await;
                    }
                }
                ResumeMsg::Need(need) => {
                    self.clone().process_need(need).await;
                }
                ResumeMsg::TasksDone { need_id } => {
                    let need = {
                        let mut active = self.active_need.lock().await;
                        active.as_mut().and_then(|n| {
                            if n.wait_kind == Some(WaitKind::Tasks) && n.need_id == need_id {
                                n.wait_kind = None;
                                n.wait_done_sent = false;
                                n.pending_task_ids.clear();
                                Some(n.clone())
                            } else {
                                None
                            }
                        })
                    };

                    if let Some(need) = need {
                        tracing::debug!(
                            head = %self.head_id,
                            need_id = %need.need_id,
                            scope = %need.scope.as_deref().unwrap_or("main"),
                            reply_to = ?need.reply_to,
                            "proc tasks done; resuming need"
                        );
                        self.clone().process_need(need).await;
                    }
                }
            }

            // Need dispatch occurs via kernel need:lease.
            // Internal task waiting resumes via ResumeMsg::TasksDone.
        }
    }

    async fn process_need(self: Arc<Self>, mut need: ActiveNeed) {
        *self.active_need.lock().await = Some(need.clone());

        tracing::debug!(
            head = %self.head_id,
            need_id = %need.need_id,
            scope = %need.scope.as_deref().unwrap_or("main"),
            reply_to = ?need.reply_to,
            "processing need"
        );

        // Process the need
        if self.llm.is_some() {
            let (summary, wait_kind, pending_task_ids) = self.think(&mut need).await;

            if let Some(kind) = wait_kind {
                // Only terminate the transport stream early for external tool calls.
                // For internal tasks, keep the stream open and resume when proc tasks complete.
                // External tool calls emit a terminal Redirect via the reply stream.

                // Leave active_need set; we will resume on proc task completion or external tool result.
                need.wait_kind = Some(kind);
                need.pending_task_ids = pending_task_ids;
                need.wait_done_sent = false;

                // Persist updated state for resumes.
                *self.active_need.lock().await = Some(need.clone());

                if kind == WaitKind::Tasks {
                    let need_id = need.need_id.clone();
                    let this = self.clone();
                    tokio::spawn(async move {
                        this.wait_for_tasks_and_resume(need_id).await;
                    });
                } else if kind == WaitKind::ExternalTool {
                    let Some(tool_call_id) = need.active_external_id.clone() else {
                        return;
                    };
                    self.arm_external_waiter(tool_call_id).await;
                }
                return;
            }

            self.fulfill_need(&need, &summary).await;
        } else {
            let msg = "Head LLM not configured. Check abbot.toml: ensure head.model (or harness.model) is set and providers.<provider>.base_url is configured.";
            self.send_error(&need, msg).await;
            self.fulfill_need(&need, "LLM not configured").await;
        }

        // Always clear active_need
        *self.active_need.lock().await = None;
    }

    async fn wait_for_tasks_and_resume(self: Arc<Self>, need_id: String) {
        use futures::future::select_all;
        use std::future::Future;
        use std::pin::Pin;

        loop {
            let pending_ids = {
                let active = self.active_need.lock().await;
                let Some(n) = active.as_ref() else {
                    return;
                };
                if n.need_id != need_id {
                    return;
                }
                if n.wait_kind != Some(WaitKind::Tasks) {
                    return;
                }
                n.pending_task_ids.clone()
            };

            if pending_ids.is_empty() {
                let _ = self
                    .resume_tx
                    .send(ResumeMsg::TasksDone {
                        need_id: need_id.clone(),
                    })
                    .await;
                return;
            }

            let mut unfinished: Vec<String> = Vec::new();
            let Some(k) = Kernel::get() else {
                return;
            };

            for id in &pending_ids {
                match k.tasks().status(id).await {
                    None => unfinished.push(id.clone()),
                    Some(crate::kernel::TaskStatus::Queued) => unfinished.push(id.clone()),
                    Some(crate::kernel::TaskStatus::Running { .. }) => unfinished.push(id.clone()),
                    Some(crate::kernel::TaskStatus::Done { .. }) => {}
                }
            }

            if unfinished.is_empty() {
                let _ = self
                    .resume_tx
                    .send(ResumeMsg::TasksDone {
                        need_id: need_id.clone(),
                    })
                    .await;
                return;
            }

            let notifies: Vec<Arc<tokio::sync::Notify>> = {
                let mut out = Vec::new();
                for id in &unfinished {
                    out.push(k.tasks().watcher(id).await);
                }
                out
            };

            let waits: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = notifies
                .into_iter()
                .map(|n| {
                    Box::pin(async move { n.notified().await })
                        as Pin<Box<dyn Future<Output = ()> + Send>>
                })
                .collect();

            let _ = select_all(waits).await;
        }
    }

    // External tool results resume via ResumeMsg delivered to the head run loop.

    async fn fulfill_need(&self, need: &ActiveNeed, summary: &str) {
        if let Some(k) = Kernel::get() {
            let dispatcher = k.dispatcher().await;
            let req = crate::kernel::Frame::req(
                "need:fulfill",
                serde_json::json!({"need_id": need.need_id.clone(), "summary": summary}),
            )
            .with_actor(format!("head/{}", self.head_id));
            let mut rx = dispatcher.dispatch(
                req,
                self.workspace_root.clone(),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx.recv().await;
        }

        tracing::debug!(
            head = %self.head_id,
            need_id = %need.need_id,
            scope = %need.scope.as_deref().unwrap_or("main"),
            reply_to = ?need.reply_to,
            "need fulfilled"
        );
    }

    async fn send_error(&self, need: &ActiveNeed, message: &str) {
        tracing::error!(
            head = %self.head_id,
            need_id = %need.need_id,
            scope = %need.scope.as_deref().unwrap_or("main"),
            "{}",
            message
        );

        if let (Some(k), Some(reply_to)) = (Kernel::get(), need.reply_to) {
            let scope = need.scope.as_deref().unwrap_or("main");
            let _ = k
                .sigcalls()
                .send(
                    scope,
                    reply_to,
                    crate::kernel::Frame::error(reply_to, json!({"message": message})),
                )
                .await;
            k.sigcalls().close(scope, reply_to).await;
        }
    }

    async fn dispatch_external_tool_call(
        &self,
        scope: &str,
        reply_to: Uuid,
        tc: &ToolCall,
    ) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };

        let rx = k
            .external_tools()
            .register_pending(scope, tc.id.as_str())
            .await
            .map_err(|e| e.to_string())?;
        self.external_waiters.lock().await.insert(tc.id.clone(), rx);

        // External tools are surfaced to the client without the internal `user__` prefix.
        let client_name = tc
            .function
            .name
            .strip_prefix("user__")
            .unwrap_or(tc.function.name.as_str())
            .to_string();

        // Emit a terminal redirect into the reply stream. This ends the transport stream
        // and tells the caller to execute the external tool.
        let _ = k
            .sigcalls()
            .send(
                scope,
                reply_to,
                crate::kernel::Frame::redirect(
                    reply_to,
                    json!({
                        "tool_call_id": tc.id.clone(),
                        "name": client_name,
                        "arguments": tc.function.arguments.clone(),
                    }),
                )
                .with_name("tool:request"),
            )
            .await;
        k.sigcalls().close(scope, reply_to).await;
        Ok(())
    }

    async fn arm_external_waiter(&self, tool_call_id: String) {
        let rx = {
            let mut waiters = self.external_waiters.lock().await;
            waiters.remove(&tool_call_id)
        };

        let Some(rx) = rx else {
            return;
        };

        let resume_tx = self.resume_tx.clone();
        tokio::spawn(async move {
            let output = match tokio::time::timeout(Duration::from_secs(300), rx).await {
                Ok(Ok(out)) => out,
                _ => return,
            };
            let _ = resume_tx
                .send(ResumeMsg::ExternalTool {
                    tool_call_id,
                    output,
                })
                .await;
        });
    }

    async fn think(&self, need: &mut ActiveNeed) -> (String, Option<WaitKind>, Vec<String>) {
        let Some(_llm) = &self.llm else {
            return ("LLM not configured".to_string(), None, Vec::new());
        };

        let snap = self.snapshot.get();
        let tools = snap.head_tools.clone();
        let plugins = snap.plugins.clone();
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
            .with_generation(self.generation.clone())
            .with_filter(self.filter.clone())
            .with_poverty(self.poverty.clone())
            .with_tars(tars);
        // Build the initial transcript once per need; resumes continue from `need.llm_messages`.
        if need.llm_messages.is_empty() {
            let mut messages = bundle_builder.build(&bundle_cfg);

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
            messages.push(crate::llm::ChatMessage::new(
                crate::llm::Role::User,
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

        let tools = tools;
        let tool_choice = serde_json::json!("auto");

        let mut final_summary = String::new();
        let mut wait_kind: Option<WaitKind> = None;
        let mut pending_task_ids: Vec<String> = Vec::new();

        let mut tools = tools;

        let (external_tools, external_names) = {
            let mut external_tools: Vec<crate::llm::ToolSpec> = Vec::new();
            let mut external_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();

            if let Ok(ext) = self.store.list_tools(&default_scope, "external") {
                for t in ext {
                    if let Ok(schema) = serde_json::from_str::<serde_json::Value>(&t.schema_json) {
                        let internal_name = format!("user__{}", t.name);
                        external_tools.push(crate::llm::ToolSpec::function(
                            internal_name.clone(),
                            // Don't include the full tool description in the LLM-facing tool spec.
                            // External tools often ship massive instructions; keep the tool list terse
                            // and require head__tool_explain when full details are needed.
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

        for iter in 0..12usize {
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
                    break;
                }
            };

            let _ = self.store.log_llm_interaction(
                "head",
                &run_id,
                iter,
                &result.request_json,
                &result.response_json,
            );

            // Log what the head decided
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
            if let Some(ref content) = result.content {
                if !content.trim().is_empty() {
                    tracing::info!(
                        head = %self.head_id,
                        scope = %default_scope,
                        reply_to = ?reply_to,
                        content = %truncate(content, 100),
                        "head says"
                    );
                }
            }

            if !result.tool_calls.is_empty() {
                if let Some(ref content) = result.content {
                    if !content.trim().is_empty() {
                        need.llm_messages.push(crate::llm::ChatMessage::new(
                            crate::llm::Role::Assistant,
                            content.clone(),
                        ));
                    }
                }

                let mut external_calls: Vec<ToolCall> = result
                    .tool_calls
                    .iter()
                    .filter(|tc| external_names.contains(&tc.function.name))
                    .cloned()
                    .collect();

                if !external_calls.is_empty() {
                    // Limit the number of external calls per LLM turn.
                    const MAX_EXTERNAL_CALLS_PER_TURN: usize = 8;
                    if external_calls.len() > MAX_EXTERNAL_CALLS_PER_TURN {
                        external_calls.truncate(MAX_EXTERNAL_CALLS_PER_TURN);
                    }

                    // Drop recently-seen signatures to avoid accidental runaway loops.
                    external_calls.retain(|tc| {
                        let sig = external_tool_sig(tc);
                        !need.recent_external_sigs.iter().any(|s| *s == sig)
                    });

                    if external_calls.is_empty() {
                        final_summary =
                            "Requested external tools, but all were recently repeated; refusing to re-run.".to_string();
                        break;
                    }

                    // IMPORTANT: only record the external tool calls we will actually execute.
                    // The upstream OpenAI API requires every tool_call_id in assistant.tool_calls
                    // to be followed by a tool message before the next assistant message.
                    need.llm_messages
                        .push(crate::llm::ChatMessage::assistant_tool_calls(external_calls.clone()));

                    for tc in &external_calls {
                        let sig = external_tool_sig(tc);
                        need.recent_external_sigs.push_back(sig);
                        const MAX_SIGS: usize = 64;
                        while need.recent_external_sigs.len() > MAX_SIGS {
                            need.recent_external_sigs.pop_front();
                        }
                    }

                    need.pending_external = external_calls;
                    let Some(first) = need.pending_external.first().cloned() else {
                        continue;
                    };
                    need.active_external_id = Some(first.id.clone());

                    let Some(parent_id) = reply_to else {
                        need.llm_messages.push(crate::llm::ChatMessage::tool_result(
                            first.id.clone(),
                            "Requested external tool, but missing reply_to for redirect correlation.".to_string(),
                        ));
                        need.pending_external.clear();
                        need.active_external_id = None;
                        continue;
                    };

                    match self
                        .dispatch_external_tool_call(default_scope.as_str(), parent_id, &first)
                        .await
                    {
                        Ok(()) => {
                            wait_kind = Some(WaitKind::ExternalTool);
                            final_summary = "Requested external tool(s); waiting for result.".to_string();
                            break;
                        }
                        Err(e) => {
                            need.llm_messages.push(crate::llm::ChatMessage::tool_result(
                                first.id.clone(),
                                format!("External tool dispatch failed: {e}"),
                            ));
                            need.pending_external.clear();
                            need.active_external_id = None;
                            continue;
                        }
                    }
                }

                // No external tools: record full tool call list and execute internally.
                need.llm_messages
                    .push(crate::llm::ChatMessage::assistant_tool_calls(result.tool_calls.clone()));

                let workspace = Workspace::new(self.workspace_root.clone());
                let cwd: SharedCwd = Arc::new(Mutex::new(self.workspace_root.clone()));

                for tc in &result.tool_calls {
                    if external_names.contains(&tc.function.name) {
                        continue;
                    }
                    if matches!(
                        tc.function.name.as_str(),
                        "head__task_create" | "head__fs_search_goal"
                    ) {
                        wait_kind = Some(WaitKind::Tasks);
                    }

                    // Acquire session write lock for mutating tools (built-in or plugin)
                    let is_mutating = if plugins.is_enabled_head_tool_name(&tc.function.name) {
                        plugins.is_plugin_mutating(&tc.function.name)
                    } else {
                        head_tool_effect(&tc.function.name)
                            .map(|e| e == ToolEffect::Mutating)
                            .unwrap_or(false)
                    };
                    let _write_guard = if is_mutating {
                        Some(self.session_locks.acquire(&default_scope).await)
                    } else {
                        None
                    };

                    let out = if plugins.is_enabled_head_tool_name(&tc.function.name) {
                        plugins
                            .exec_head_tool(
                                &workspace,
                                &cwd,
                                &tc.function.name,
                                &tc.function.arguments,
                            )
                            .await
                    } else {
                        exec_head_tool(
                            self.store.as_ref(),
                            Some(&workspace),
                            Some(&cwd),
                            &self.head_id,
                            &default_scope,
                            reply_to,
                            self.memory.as_ref(),
                            self.ems.as_ref(),
                            &format!("head/{}", self.head_id),
                            &tc.function.name,
                            &tc.function.arguments,
                        )
                        .await
                    };

                    if matches!(
                        tc.function.name.as_str(),
                        "head__task_create" | "head__fs_search_goal"
                    ) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&out) {
                            if v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false) {
                                if let Some(task_id) = v
                                    .get("data")
                                    .and_then(|d| d.get("task_id"))
                                    .and_then(|t| t.as_str())
                                {
                                    if !pending_task_ids.iter().any(|id| id == task_id) {
                                        pending_task_ids.push(task_id.to_string());
                                    }
                                }
                            }
                        }
                    }

                    need.llm_messages
                        .push(crate::llm::ChatMessage::tool_result(tc.id.clone(), out));
                }

                if wait_kind == Some(WaitKind::Tasks) {
                    final_summary = "Queued tasks; waiting for completion.".to_string();
                    break;
                }
                continue;
            }

            // No tool calls - this is the final response
            let content = result.content.unwrap_or_default();
            if !content.trim().is_empty() {
                need.llm_messages.push(crate::llm::ChatMessage::new(
                    crate::llm::Role::Assistant,
                    content.clone(),
                ));
                if let (Some(k), Some(r)) = (Kernel::get(), reply_to) {
                    let _ = k
                        .sigcalls()
                        .send(
                            default_scope.as_str(),
                            r,
                            crate::kernel::Frame::bytes(
                                r,
                                json!({"text": format!("{}\n", content)}),
                            )
                            .with_name("chat:message"),
                        )
                        .await;
                    let _ = k
                        .sigcalls()
                        .send(default_scope.as_str(), r, crate::kernel::Frame::done(r))
                        .await;
                    k.sigcalls().close(default_scope.as_str(), r).await;
                }

                final_summary = truncate(&content, 200);
            } else {
                final_summary = "Completed without response".to_string();
            }
            break;
        }

        (final_summary, wait_kind, pending_task_ids)
    }

    async fn chat_head_llm_with_fallback(
        &self,
        scope: &str,
        messages: Vec<crate::llm::ChatMessage>,
        tools: Vec<crate::llm::ToolSpec>,
        tool_choice: serde_json::Value,
    ) -> Result<crate::llm::ChatToolResult, super::llm_harness::HarnessError> {
        let Some(k) = Kernel::get() else {
            return Err(super::llm_harness::HarnessError {
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

        while let Some(frame) = rx.recv().await {
            match frame.op {
                crate::kernel::FrameOp::Ok => {
                    let data = frame.data.unwrap_or(serde_json::Value::Null);
                    let content = data
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let tool_calls_v = data.get("tool_calls").cloned().unwrap_or_else(|| json!([]));
                    let tool_calls: Vec<ToolCall> = serde_json::from_value(tool_calls_v).map_err(|e| {
                        super::llm_harness::HarnessError {
                            message: format!("invalid llm tool_calls: {e}"),
                        }
                    })?;

                    let usage_v = data.get("usage").cloned().unwrap_or(serde_json::Value::Null);
                    let usage: Option<crate::llm::Usage> = serde_json::from_value(usage_v).map_err(|e| {
                        super::llm_harness::HarnessError {
                            message: format!("invalid llm usage: {e}"),
                        }
                    })?;

                    let request_json = data
                        .get("request_json")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let response_json = data
                        .get("response_json")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    return Ok(crate::llm::ChatToolResult {
                        content,
                        tool_calls,
                        usage,
                        request_json,
                        response_json,
                    });
                }
                crate::kernel::FrameOp::Error => {
                    let msg = frame
                        .data
                        .as_ref()
                        .and_then(|d| d.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("llm syscall error")
                        .to_string();

                    return Err(super::llm_harness::HarnessError { message: msg });
                }
                _ => {}
            }
        }

        Err(super::llm_harness::HarnessError {
            message: format!("llm:chat ended without response (scope={scope})"),
        })
    }
}

fn external_tool_sig(tc: &ToolCall) -> u64 {
    let mut hasher = DefaultHasher::new();
    tc.function.name.hash(&mut hasher);
    tc.function.arguments.hash(&mut hasher);
    hasher.finish()
}

fn truncate(s: &str, max: usize) -> String {
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
