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
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::bus::{Message, MessageData, MessageOp, NeedMsg, Origin, Scope, respond};
use crate::history::Store;
use crate::llm::OpenAICompatClient;
use crate::recall::Search;
use crate::agent_tools::{exec_head_tool, Workspace, SharedCwd};
use crate::runtime::AppConfig;
use crate::runtime::summarize_tool_args;
use crate::runtime::models_config::ModelsConfig;
use super::llm_harness::{chat_with_tools_retry, RetryPolicy};

use super::{HeadBundleBuilder, HeadBundleConfig, HeadConfig, RuntimeBus, GenerationMode, SnapshotManager};

// Context for the need currently being processed
#[derive(Debug, Clone)]
struct ActiveNeed {
    need_id: String,
    need_text: String,
    context: String,
    reply_to: Option<Uuid>,

    // Local state for multi-step execution.
    waiting_on_tasks: bool,
    wait_done_sent: bool,
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
}

fn head_context_budget_tokens() -> Option<u32> {
    let model_id = std::env::var("HEAD_MODEL")
        .ok()
        .or_else(|| AppConfig::global().head.llm.model.clone())?;

    let ctx = ModelsConfig::global().get(&model_id)?.context_window?;
    Some(ctx / 2)
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

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
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

            // Handle need dispatch from NeedService
            if let (MessageOp::Need, MessageData::Need(NeedMsg::Acknowledged { need_id, head_id })) =
                (&msg.op, &msg.data)
            {
                if head_id == &self.head_id {
                    tracing::debug!(
                        head = %self.head_id,
                        need_id = %need_id,
                        "need acknowledged"
                    );
                }
                continue;
            }

            // Handle need content (sent as chat from need_service after acknowledgment)
            if msg.op == MessageOp::Chat
                && msg.origin == Origin::System
                && msg.sender == "need_service"
            {
                if let Some(need) = self.parse_need_content(&msg) {
                    self.process_need(need).await;
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
                                    if n.waiting_on_tasks {
                                        n.waiting_on_tasks = false;
                                        n.wait_done_sent = false;
                                        Some(n.clone())
                                    } else {
                                        None
                                    }
                                })
                        };

                        if let Some(need) = need {
                            tracing::debug!(head = %self.head_id, need_id = %need.need_id, "tasks drained; resuming need");
                            let this = self.clone();
                            tokio::spawn(async move { this.process_need(need).await; });
                        } else {
                            tracing::debug!(head = %self.head_id, "tasks drained notification received");
                        }
                    }
                }
            }
        }
    }

    async fn process_need(&self, need: ActiveNeed) {
        *self.active_need.lock().await = Some(need.clone());

        // Process the need
        if self.llm.is_some() {
            let (summary, waiting_on_tasks) = self.think(&need).await;

            if waiting_on_tasks {
                let default_scope = self
                    .scopes
                    .first()
                    .map(|s| s.to_string())
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

                if should_emit_done {
                    let mut done = respond::done(&self.head_id, Scope::from(default_scope.as_str()))
                        .with_origin(Origin::Head);
                    if let Some(r) = need.reply_to {
                        done = done.with_reply_to(r);
                    }
                    self.bus.publish(done).await;
                }

                // Leave active_need set; we will resume on tasks_drained.
                let mut active = self.active_need.lock().await;
                if let Some(n) = active.as_mut() {
                    n.waiting_on_tasks = true;
                }
                return;
            }

            self.fulfill_need(&need, &summary).await;
        }

        // Always clear active_need (even if LLM not configured)
        *self.active_need.lock().await = None;
    }

    fn parse_need_content(&self, msg: &Message) -> Option<ActiveNeed> {
        let text = msg.text()?;

        // Parse the format: [need_id=X] [source=Y] [priority=Z]\nNeed text\n\nContext: ...
        let mut need_id = None;
        let mut need_text = String::new();
        let mut context = String::new();

        for line in text.lines() {
            if line.starts_with("[need_id=") {
                // Extract need_id from [need_id=X]
                if let Some(start) = line.find("[need_id=") {
                    if let Some(end) = line[start..].find(']') {
                        need_id = Some(line[start + 9..start + end].to_string());
                    }
                }
            } else if line.starts_with("Context: ") {
                context = line.strip_prefix("Context: ").unwrap_or("").to_string();
            } else if !line.starts_with('[') && !line.is_empty() {
                if !need_text.is_empty() {
                    need_text.push('\n');
                }
                need_text.push_str(line);
            }
        }

        Some(ActiveNeed {
            need_id: need_id?,
            need_text,
            context,
            reply_to: msg.reply_to,
            waiting_on_tasks: false,
            wait_done_sent: false,
        })
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
            "need fulfilled"
        );
    }

    async fn think(&self, need: &ActiveNeed) -> (String, bool) {
        let Some(llm) = &self.llm else {
            return ("LLM not configured".to_string(), false);
        };

        let snap = self.snapshot.get();
        let tools = snap.head_tools.clone();
        let plugins = snap.plugins.clone();
        let bundle_builder = HeadBundleBuilder::new_with_snapshot(
            self.store.clone(),
            self.workspace_root.clone(),
            self.snapshot.clone(),
        );
        let bundle_cfg = HeadBundleConfig::new(&self.head_id, self.scopes.clone())
            .with_context_budget_tokens(head_context_budget_tokens())
            .with_generation(self.generation.clone());
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

        let default_scope = self.scopes.first()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "main".to_string());

        let run_id = format!("need:{}", need.need_id);
        let reply_to = need.reply_to;

        let tools = tools;
        let tool_choice = serde_json::json!("auto");
        let policy = RetryPolicy::default_llm();

        let mut final_summary = String::new();
        let mut waiting_on_tasks = false;

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
                        "head tool call"
                    );
                }
            }
            if let Some(ref content) = result.content {
                if !content.trim().is_empty() {
                    tracing::info!(head = %self.head_id, content = %truncate(content, 100), "head says");
                }
            }

            if !result.tool_calls.is_empty() {
                messages.push(crate::llm::ChatMessage::assistant_tool_calls(
                    result.tool_calls.clone(),
                ));

                let workspace = Workspace::new(self.workspace_root.clone());
                let cwd: SharedCwd = Arc::new(Mutex::new(self.workspace_root.clone()));

                for tc in &result.tool_calls {
                    if tc.function.name == "create_task" || tc.function.name == "search_files_goal" {
                        waiting_on_tasks = true;
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

                if waiting_on_tasks {
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

                final_summary = truncate(&content, 200);
            } else {
                final_summary = "Completed without response".to_string();
            }
            break;
        }

        (final_summary, waiting_on_tasks)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
