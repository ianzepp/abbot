//! HeadService - AI agent that processes needs via LLM and tool execution
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! HeadService is the core AI agent in Abbot's runtime. It leases needs from the
//! NeedKernel, builds context from the message store, calls an LLM with tool access,
//! and executes tool calls (internal and external). This is the "brain" of the system,
//! where chat turns and autonomous needs are fulfilled.
//!
//! The head is purely reactive and stateless between needs. It does not watch scopes
//! or maintain persistent state beyond the active need. All context is rebuilt from
//! the message store on each need lease, enabling restart without data loss.
//!
//! After the syscall refactor, HeadService dispatches all chat and turn lifecycle
//! operations via syscalls (chat:message, chat:tool, chat:done, chat:error) rather
//! than directly emitting frames. This ensures consistent turn semantics and enables
//! kernel-owned turn cancellation.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Stateless between needs: No persistent state beyond the active need.
//!   All context is rebuilt from the message store on each lease.
//! - Syscall-driven output: All chat messages, tool calls, and lifecycle events
//!   are dispatched as syscalls, not emitted as raw frames.
//! - External tool continuations: External tools pause the need and resume it
//!   when results arrive, preserving the LLM transcript across segments.
//! - Internal tool execution: Internal tools (head__ prefix) execute synchronously
//!   within the same need processing loop without pausing.
//! - Cancellation checkpoints: The head checks for turn cancellation before LLM
//!   calls, external tool dispatch, and need fulfillment.
//!
//! TRADE-OFFS
//! ==========
//! - Stateless vs persistent context: We chose stateless to enable crash recovery
//!   and horizontal scaling. The cost is rebuilding context from the store on each
//!   lease, which adds latency.
//! - Syscall overhead vs direct emission: Syscalls add a dispatch layer compared to
//!   direct frame emission. The benefit is consistent turn semantics and kernel-owned
//!   cancellation/rendezvous for external tools.
//! - External tool pause/resume: External tools close the HTTP segment and resume
//!   later. This enables long-running client-side operations but requires transcript
//!   persistence and rendezvous state.
//!
//! CONCURRENCY
//! ===========
//! Each head runs in its own tokio task. The active_need field is behind a tokio
//! Mutex to coordinate between the main loop and resume channels. Session write
//! locks prevent concurrent mutating tool execution within a session scope.

mod types;
mod config;
mod bundle;
mod dispatch;
mod resume;
mod think;

// Re-exports for runtime/mod.rs
pub use bundle::{GenerationMode, HeadBundleBuilder, HeadBundleConfig};
pub use config::HeadConfig;

use types::{ActiveNeed, WaitKind, ResumeMsg};
use think::truncate;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::Scope;
use crate::ems::EmsHandle;
use crate::history::Store;
use crate::hal::llm::OpenAICompatClient;
use crate::runtime::Kernel;
use crate::runtime::{SessionWriteLocks, SnapshotManager};
/// HeadService is the AI agent that processes needs.
pub struct HeadService {
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,
    llm: Option<Arc<OpenAICompatClient>>,
    workspace_root: PathBuf,
    snapshot: Arc<SnapshotManager>,
    active_need: tokio::sync::Mutex<Option<ActiveNeed>>,
    resume_tx: mpsc::Sender<ResumeMsg>,
    resume_rx: tokio::sync::Mutex<Option<mpsc::Receiver<ResumeMsg>>>,
    generation: GenerationMode,
    filter: crate::runtime::FilterMode,
    poverty: crate::runtime::PovertyMode,
    session_locks: SessionWriteLocks,
    ems: Option<EmsHandle>,
}

impl HeadService {
    /// Create a new HeadService.
    pub fn new(
        store: Arc<Store>,
        workspace_root: PathBuf,
        head_id: impl Into<String>,
        scopes: Vec<Scope>,
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
            store,
            head_id,
            scopes,
            llm,
            workspace_root,
            snapshot,
            active_need: tokio::sync::Mutex::new(None),
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

    /// Main run loop: lease needs, process them, resume on tool results.
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
                        recent_external_sigs: VecDeque::new(),
                    };

                    let _ = resume_tx.send(ResumeMsg::Need(need)).await;
                });
            }

            let resume = resume_rx.recv().await;
            let Some(resume) = resume else {
                return;
            };

            match resume {
                ResumeMsg::ExternalTools { results } => {
                    let resume_need = {
                        let mut active = self.active_need.lock().await;
                        let Some(n) = active.as_mut() else {
                            continue;
                        };

                        if n.wait_kind != Some(WaitKind::ExternalTool) {
                            continue;
                        }

                        for result in results {
                            const MAX_TOOL_OUTPUT_CHARS: usize = 20_000;
                            let output = truncate(&result.content, MAX_TOOL_OUTPUT_CHARS);
                            n.llm_messages.push(crate::hal::llm::ChatMessage::tool_result(
                                result.tool_call_id,
                                output,
                            ));
                        }

                        n.pending_external.clear();
                        n.wait_kind = None;
                        Some(n.clone())
                    };

                    if let Some(need) = resume_need {
                        tracing::debug!(
                            head = %self.head_id,
                            need_id = %need.need_id,
                            scope = %need.scope.as_deref().unwrap_or("main"),
                            reply_to = ?need.reply_to,
                            "external tools completed; resuming need"
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
        }
    }

    /// Process a need: call LLM, execute tools, emit chat, handle waits.
    async fn process_need(self: Arc<Self>, mut need: ActiveNeed) {
        *self.active_need.lock().await = Some(need.clone());

        tracing::debug!(
            head = %self.head_id,
            need_id = %need.need_id,
            scope = %need.scope.as_deref().unwrap_or("main"),
            reply_to = ?need.reply_to,
            "processing need"
        );

        if self.llm.is_some() {
            let (summary, wait_kind, pending_task_ids) = self.think(&mut need).await;

            if let Some(kind) = wait_kind {
                need.wait_kind = Some(kind);
                need.pending_task_ids = pending_task_ids;

                *self.active_need.lock().await = Some(need.clone());

                if kind == WaitKind::Tasks {
                    let need_id = need.need_id.clone();
                    let this = self.clone();
                    tokio::spawn(async move {
                        this.wait_for_tasks_and_resume(need_id).await;
                    });
                } else if kind == WaitKind::ExternalTool {
                    let pending = need.pending_external.clone();
                    let this = self.clone();
                    tokio::spawn(async move {
                        this.wait_for_external_tools_and_resume(pending).await;
                    });
                }
                return;
            }

            if self.is_turn_cancelled(&need).await {
                tracing::debug!(
                    head = %self.head_id,
                    need_id = %need.need_id,
                    scope = %need.scope.as_deref().unwrap_or("main"),
                    reply_to = ?need.reply_to,
                    "turn cancelled before need fulfill"
                );
            }
            self.fulfill_need(&need, &summary).await;
        } else {
            let msg = "Head LLM not configured. Check abbot.toml: ensure head.model (or harness.model) is set and providers.<provider>.base_url is configured.";
            self.send_error(&need, msg).await;
            self.fulfill_need(&need, "LLM not configured").await;
        }

        *self.active_need.lock().await = None;
    }
}
