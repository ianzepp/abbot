// HeadService is the AI decision-maker that converts needs into tasks.
//
// Heads are purely reactive - they don't watch scopes directly. Instead, they
// receive needs from NeedService (dispatched to their mailbox) and process them
// by calling an LLM that can create tasks, send chat messages, etc. When done
// processing a need, the head emits NeedMsg::Fulfilled.
//
// The head is intentionally stateless between needs - all context comes from
// the message store, enabling restart without data loss.

use std::path::PathBuf;
use std::time::Duration;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};

use uuid::Uuid;

use crate::bus::{MessageData, MessageOp, NeedMsg, Origin, Scope, respond};
use crate::history::Store;
use crate::llm::{OpenAICompatClient, ToolCall};
use crate::recall::Search;
use crate::agent_tools::{exec_head_tool, Workspace, SharedCwd};
use crate::runtime::AppConfig;
use crate::runtime::summarize_tool_args;
use crate::runtime::models_config::ModelsConfig;
use super::llm_harness::{chat_with_tools_retry, RetryPolicy};

use super::{HeadBundleBuilder, HeadBundleConfig, HeadConfig, RuntimeBus, GenerationMode, SnapshotManager, TarsDials, sandbox_config_from_workspace_root, read_optional_file};

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
    pending_tool_call: Option<ToolCall>,
    pending_tool_output: Option<String>,
    wait_done_sent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitKind {
    Tasks,
    ExternalTool,
}

pub struct HeadService {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,  // Scopes this head can read context from
    memory: Option<Arc<Search>>,
    llm: Option<Arc<OpenAICompatClient>>,
    workspace_root: PathBuf,
    snapshot: Arc<SnapshotManager>,
    active_need: tokio::sync::Mutex<Option<ActiveNeed>>,
    generation: GenerationMode,

    heartbeat_tick: Duration,
    idle_tick_enabled: bool,
    tick: AtomicU64,
}

fn parse_bool_env(key: &str) -> bool {
    let Ok(v) = std::env::var(key) else {
        return false;
    };
    let v = v.trim().to_ascii_lowercase();
    matches!(v.as_str(), "1" | "true" | "yes" | "y" | "on")
}

fn head_context_budget_tokens() -> Option<u32> {
    let model_id = std::env::var("HEAD_MODEL")
        .ok()
        .or_else(|| AppConfig::global().head.llm.model.clone())?;

    let ctx = ModelsConfig::global().get(&model_id)?.context_window?;
    Some(ctx / 2)
}

fn load_tars_dials(workspace_root: &std::path::Path) -> TarsDials {
    let Some(config_path) = sandbox_config_from_workspace_root(workspace_root) else {
        return TarsDials::default();
    };

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
        humor: tars.get("humor").and_then(|v| v.as_float()).map(|f| f as f32),
        honesty: tars.get("honesty").and_then(|v| v.as_float()).map(|f| f as f32),
        sarcasm: tars.get("sarcasm").and_then(|v| v.as_float()).map(|f| f as f32),
        verbosity: tars.get("verbosity").and_then(|v| v.as_float()).map(|f| f as f32),
        confidence: tars.get("confidence").and_then(|v| v.as_float()).map(|f| f as f32),
        curiosity: tars.get("curiosity").and_then(|v| v.as_float()).map(|f| f as f32),
        patience: tars.get("patience").and_then(|v| v.as_float()).map(|f| f as f32),
        formality: tars.get("formality").and_then(|v| v.as_float()).map(|f| f as f32),
        empathy: tars.get("empathy").and_then(|v| v.as_float()).map(|f| f as f32),
        pedantry: tars.get("pedantry").and_then(|v| v.as_float()).map(|f| f as f32),
        initiative: tars.get("initiative").and_then(|v| v.as_float()).map(|f| f as f32),
        optimism: tars.get("optimism").and_then(|v| v.as_float()).map(|f| f as f32),
        caution: tars.get("caution").and_then(|v| v.as_float()).map(|f| f as f32),
    }
}

impl HeadService {
    pub fn new(
        bus: RuntimeBus,
        store: Arc<Store>,
        head_id: impl Into<String>,
        scopes: Vec<Scope>,
        memory: Option<Arc<Search>>,
        snapshot: Arc<SnapshotManager>,
    ) -> Self {
        let head_id = head_id.into();
        let head_cfg = HeadConfig::from_env();

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
            None
        };

        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            bus,
            store,
            head_id,
            scopes,
            memory,
            llm,
            workspace_root,
            snapshot,
            active_need: tokio::sync::Mutex::new(None),
            generation: GenerationMode::None,

            heartbeat_tick: Duration::from_secs(head_cfg.heartbeat_tick.max(1)),
            idle_tick_enabled: parse_bool_env("HEAD_IDLE_TICK"),
            tick: AtomicU64::new(0),
        }
    }

    pub fn with_generation(mut self, generation: GenerationMode) -> Self {
        self.generation = generation;
        self
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(self: Arc<Self>) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        let my_mailbox = Scope::head_mail(&self.head_id);
        tracing::debug!(head = %self.head_id, mailbox = %my_mailbox, "head service started");

        let mut interval = tokio::time::interval(self.heartbeat_tick);

        loop {
            let maybe_msg = tokio::select! {
                _ = interval.tick() => {
                    if self.idle_tick_enabled {
                        let idle = self.active_need.lock().await.is_none();
                        if idle {
                            let tick = self.tick.fetch_add(1, Ordering::Relaxed) + 1;
                            self.bus
                                .publish(
                                    respond::wake(&self.head_id, Scope::main(), tick)
                                        .with_origin(Origin::Head),
                                )
                                .await;
                        }
                    }
                    None
                }

                msg = rx.recv() => msg.ok(),
            };

            let Some(msg) = maybe_msg else {
                continue;
            };

            if msg.op == MessageOp::Event && msg.origin == Origin::System {
                if let MessageData::Event { kind, .. } = &msg.data {
                    if kind == "collective_reboot" {
                        self.snapshot.refresh();
                        tracing::info!(head = %self.head_id, "refreshed runtime snapshot (collective reboot)");
                        continue;
                    }
                }
            }

            // Only process messages to our mailbox
            if msg.scope != my_mailbox {
                continue;
            }

            if let (MessageOp::Need, MessageData::Need(NeedMsg::Dispatch {
                need_id,
                head_id,
                source: _,
                priority: _,
                need,
                context,
                scope,
            })) = (&msg.op, &msg.data)
            {
                if head_id == &self.head_id {
                    let need = ActiveNeed {
                        need_id: need_id.clone(),
                        need_text: need.clone(),
                        context: context.clone(),
                        scope: Some(scope.clone()),
                        reply_to: msg.reply_to,
                        wait_kind: None,
                        pending_tool_call: None,
                        pending_tool_output: None,
                        wait_done_sent: false,
                    };
                    let this = self.clone();
                    tokio::spawn(async move { this.process_need(need).await; });
                }
                continue;
            }

            // Handle tasks_drained event (hand work completed)
            if msg.op == MessageOp::Event && msg.origin == Origin::System {
                if let MessageData::Event { kind, .. } = &msg.data {
                    if kind == "tasks_drained" {
                        let need = {
                            let mut active = self.active_need.lock().await;
                            active
                                .as_mut()
                                .and_then(|n| {
                                    if n.wait_kind == Some(WaitKind::Tasks) {
                                        n.wait_kind = None;
                                        n.wait_done_sent = false;
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
                                "tasks drained; resuming need"
                            );
                            let this = self.clone();
                            tokio::spawn(async move { this.process_need(need).await; });
                        } else {
                            tracing::debug!(head = %self.head_id, "tasks drained notification received");
                        }
                    }
                }
            }

            // Handle external tool results (from OpenAI compat clients)
            if msg.op == MessageOp::Event {
                if let MessageData::Event { kind, payload } = &msg.data {
                    if kind == "external_tool_result" {
                        let tool_call_id = payload
                            .get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let output = payload
                            .get("output")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        if tool_call_id.is_empty() {
                            continue;
                        }

                        let need = {
                            let mut active = self.active_need.lock().await;
                            active
                                .as_mut()
                                .and_then(|n| {
                                    if n.wait_kind == Some(WaitKind::ExternalTool) {
                                        if let Some(tc) = &n.pending_tool_call {
                                            if tc.id == tool_call_id {
                                                n.pending_tool_output = Some(output);
                                                n.wait_kind = None;
                                                n.wait_done_sent = false;
                                                return Some(n.clone());
                                            }
                                        }
                                    }
                                    None
                                })
                        };

                        if let Some(need) = need {
                            tracing::debug!(
                                head = %self.head_id,
                                need_id = %need.need_id,
                                scope = %need.scope.as_deref().unwrap_or("main"),
                                reply_to = ?need.reply_to,
                                "external tool result received; resuming need"
                            );
                            let this = self.clone();
                            tokio::spawn(async move { this.process_need(need).await; });
                        }
                    }
                }
            }
        }
    }

    async fn process_need(&self, need: ActiveNeed) {
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
            let (summary, wait_kind, pending_tool_call) = self.think(&need).await;

            if let Some(kind) = wait_kind {
                let default_scope = need
                    .scope
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .or_else(|| self.scopes.first().map(|s| s.to_string()))
                    .unwrap_or_else(|| "main".to_string());

                let should_emit_done = {
                    let mut active = self.active_need.lock().await;
                    match active.as_mut() {
                        None => false,
                        Some(n) => {
                            if n.wait_done_sent {
                                false
                            } else {
                                n.wait_done_sent = true;
                                true
                            }
                        }
                    }
                };

                // Only terminate the transport stream early for external tool calls.
                // For internal tasks, keep the stream open and resume when tasks_drained fires.
                if should_emit_done && kind == WaitKind::ExternalTool {
                    let mut done = respond::done(&self.head_id, Scope::from(default_scope.as_str()))
                        .with_origin(Origin::Head);
                    if let Some(r) = need.reply_to {
                        done = done.with_reply_to(r);
                    }
                    self.bus.publish(done).await;
                }

                // Leave active_need set; we will resume on tasks_drained or external_tool_result.
                let mut active = self.active_need.lock().await;
                if let Some(n) = active.as_mut() {
                    n.wait_kind = Some(kind);
                    n.pending_tool_call = pending_tool_call;
                    n.pending_tool_output = None;
                }
                return;
            }

            self.fulfill_need(&need, &summary).await;
        }

        // Always clear active_need (even if LLM not configured)
        *self.active_need.lock().await = None;
    }

    async fn fulfill_need(&self, need: &ActiveNeed, summary: &str) {
        let mut msg = respond::need_fulfilled(
            &self.head_id,
            Scope::from("@need_service"),
            &need.need_id,
            &self.head_id,
            summary,
        )
        .with_origin(Origin::Head);

        if let Some(reply_to) = need.reply_to {
            msg = msg.with_reply_to(reply_to);
        }

        self.bus.publish(msg).await;

        tracing::debug!(
            head = %self.head_id,
            need_id = %need.need_id,
            scope = %need.scope.as_deref().unwrap_or("main"),
            reply_to = ?need.reply_to,
            "need fulfilled"
        );
    }

    async fn think(&self, need: &ActiveNeed) -> (String, Option<WaitKind>, Option<ToolCall>) {
        let Some(llm) = &self.llm else {
            return ("LLM not configured".to_string(), None, None);
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
            .with_generation(self.generation.clone())
            .with_tars(tars);
        let mut messages = bundle_builder.build(&bundle_cfg);

        // Inject the need as a user message
        let need_prompt = format!(
            "You have been assigned a need to address:\n\n{}\n\nContext: {}",
            need.need_text,
            if need.context.is_empty() { "(none)" } else { &need.context }
        );
        messages.push(crate::llm::ChatMessage::new(crate::llm::Role::User, need_prompt));

        tracing::debug!(
            head = %self.head_id,
            need_id = %need.need_id,
            message_count = messages.len(),
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
        let policy = RetryPolicy::default_llm();

        let mut final_summary = String::new();
        let mut wait_kind: Option<WaitKind> = None;
        let mut pending_tool_call: Option<ToolCall> = None;

        // If we are resuming from an external tool call, inject the tool call + result
        // into the LLM transcript for continuity.
        if let (Some(tc), Some(out)) = (&need.pending_tool_call, &need.pending_tool_output) {
            messages.push(crate::llm::ChatMessage::assistant_tool_calls(vec![tc.clone()]));
            messages.push(crate::llm::ChatMessage::tool_result(tc.id.clone(), out.clone()));
        }

        let mut tools = tools;
        let external_tools = &snap.external_tools;
        let external_names = &snap.external_tool_names;
        let external_name_map = &snap.external_name_map;
        tools.extend(external_tools.iter().cloned());

        for iter in 0..12usize {
            let result = match chat_with_tools_retry(
                self.store.as_ref(),
                "head",
                &run_id,
                iter,
                llm.as_ref(),
                messages.clone(),
                tools.clone(),
                tool_choice.clone(),
                policy.clone(),
                |attempt, note| {
                    tracing::warn!(head = %self.head_id, attempt, note, "head llm temporary error; retrying");
                },
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
                messages.push(crate::llm::ChatMessage::assistant_tool_calls(
                    result.tool_calls.clone(),
                ));

                let workspace = Workspace::new(self.workspace_root.clone());
                let cwd: SharedCwd = Arc::new(Mutex::new(self.workspace_root.clone()));

                for tc in &result.tool_calls {
                    if external_names.contains(&tc.function.name) {
                        let client_name = external_name_map
                            .get(&tc.function.name)
                            .cloned()
                            .unwrap_or_else(|| tc.function.name.clone());
                        let mut evt = respond::event(
                            &self.head_id,
                            Scope::from(default_scope.as_str()),
                            "external_tool_request",
                            serde_json::json!({
                                "tool_call_id": tc.id.clone(),
                                "name": client_name,
                                "arguments": tc.function.arguments.clone(),
                            }),
                        )
                        .with_origin(Origin::Head);
                        if let Some(r) = reply_to {
                            evt = evt.with_reply_to(r);
                        }
                        self.bus.publish(evt).await;

                        wait_kind = Some(WaitKind::ExternalTool);
                        pending_tool_call = Some(tc.clone());
                        final_summary = "Requested external tool; waiting for result.".to_string();
                        break;
                    }
                }

                if wait_kind == Some(WaitKind::ExternalTool) {
                    break;
                }

                for tc in &result.tool_calls {
                    if matches!(
                        tc.function.name.as_str(),
                        "create_task" | "tasks_create" | "search_files_goal" | "goals_create_fs_search"
                    ) {
                        wait_kind = Some(WaitKind::Tasks);
                    }
                    let out = if plugins.is_enabled_head_tool_name(&tc.function.name) {
                        plugins
                            .exec_head_tool(&workspace, &cwd, &tc.function.name, &tc.function.arguments)
                            .await
                    } else {
                        exec_head_tool(
                            &self.bus,
                            self.store.as_ref(),
                            Some(&workspace),
                            Some(&cwd),
                            &self.head_id,
                            &default_scope,
                            reply_to,
                            self.memory.as_ref(),
                            &tc.function.name,
                            &tc.function.arguments,
                        )
                        .await
                    };
                    messages.push(crate::llm::ChatMessage::tool_result(tc.id.clone(), out));
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
                // Post the response to the default scope
                let mut chat = respond::chat(&self.head_id, Scope::from(default_scope.as_str()), &content)
                    .with_origin(Origin::Head);
                if let Some(r) = reply_to {
                    chat = chat.with_reply_to(r);
                }
                self.bus.publish(chat).await;

                // Explicitly terminate the stream for this reply chain.
                let mut done = respond::done(&self.head_id, Scope::from(default_scope.as_str()))
                    .with_origin(Origin::Head);
                if let Some(r) = reply_to {
                    done = done.with_reply_to(r);
                }
                self.bus.publish(done).await;

                final_summary = truncate(&content, 200);
            } else {
                final_summary = "Completed without response".to_string();
            }
            break;
        }

        (final_summary, wait_kind, pending_tool_call)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
