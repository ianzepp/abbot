// HeadService is the AI decision-maker that processes chat and creates tasks.
//
// Each head watches specific scopes (usually #channels) and responds to messages
// from humans. It bundles recent conversation history and sends it to an LLM,
// which returns structured actions (chat, mail, or goal creation). The head
// is intentionally stateless between triggers - all context comes from the
// message store, enabling restart without data loss.

use std::sync::Arc;
use std::time::Instant;

use tokio::time::timeout;
use uuid::Uuid;

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope, respond};
use crate::history::Store;
use crate::llm::OpenAICompatClient;
use crate::memory::Search;
use crate::agent_tools::{head_tool_specs, exec_head_tool};
use super::llm_harness::{chat_with_tools_retry, RetryPolicy};

use super::{HeadBundleBuilder, HeadBundleConfig, HeadConfig, RuntimeBus};

pub struct HeadService {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,
    memory: Option<Arc<Search>>,
    head_cfg: HeadConfig,
    llm: Option<Arc<OpenAICompatClient>>,
    pending_ctx: tokio::sync::Mutex<Option<TriggerContext>>,
}

impl HeadService {
    pub fn new(
        bus: RuntimeBus,
        store: Arc<Store>,
        head_id: impl Into<String>,
        scopes: Vec<Scope>,
        memory: Option<Arc<Search>>,
    ) -> Self {
        let head_id = head_id.into();
        let head_cfg = HeadConfig::from_env();

        let llm = if head_cfg.llm.enabled {
            tracing::info!(
                head = %head_id,
                base_url = %head_cfg.llm.base_url,
                model = %head_cfg.llm.model,
                api_key_set = !head_cfg.llm.api_key.is_empty(),
                temperature = ?head_cfg.llm.temperature,
                max_tokens = ?head_cfg.llm.max_tokens,
                heartbeat_tick = head_cfg.heartbeat_tick,
                "head llm enabled via HEAD_* env"
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

        Self {
            bus,
            store,
            head_id,
            scopes,
            memory,
            head_cfg,
            llm,
            pending_ctx: tokio::sync::Mutex::new(None),
        }
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(&self) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        tracing::info!(head = %self.head_id, "head service started");

        let mut last_think = Instant::now();
        let mut pending_think = false;

        loop {
            let recv_timeout = if pending_think {
                self.head_cfg.debounce_interval
            } else {
                std::time::Duration::from_secs(60)
            };

            let msg = match timeout(recv_timeout, rx.recv()).await {
                Ok(Ok(m)) => Some(m),
                Ok(Err(_)) => continue,
                Err(_) => None,
            };

            if let Some(msg) = msg {
                let trigger = self.handle_message(&msg).await;

                // Only trigger thinking for actual messages, not heartbeats
                if let Trigger::Message(ctx) = &trigger {
                    *self.pending_ctx.lock().await = Some(ctx.clone());
                    if self.llm.is_some() {
                        pending_think = true;
                    }
                }
            }

            let should_think =
                pending_think && last_think.elapsed() >= self.head_cfg.debounce_interval;

            if should_think {
                let ctx = self.pending_ctx.lock().await.take();
                tracing::info!(head = %self.head_id, "head thinking...");
                self.think(ctx).await;
                last_think = Instant::now();
                pending_think = false;
            }
        }
    }

    async fn handle_message(&self, msg: &Message) -> Trigger {
        if msg.op == MessageOp::Wake {
            if msg.scope.is_head_mail() && msg.scope.head_id() == Some(&self.head_id) {
                return Trigger::Heartbeat;
            }
            return Trigger::None;
        }

        if msg.op == MessageOp::Event && self.scopes.contains(&msg.scope) {
            let MessageData::Event { kind, .. } = &msg.data else {
                return Trigger::None;
            };

            // Allow GoalService to signal "all goals done for this scope".
            if msg.origin == Origin::System
                && msg.sender == "goal_service"
                && kind == "goals_drained"
            {
                let scope = match &msg.data {
                    MessageData::Event { payload, .. } => payload
                        .get("scope")
                        .and_then(|v| v.as_str())
                        .map(Scope::from)
                        .unwrap_or_else(|| msg.scope.clone()),
                    _ => msg.scope.clone(),
                };

                let msg_id = msg.reply_to.unwrap_or(msg.id);
                return Trigger::Message(TriggerContext { msg_id, scope });
            }
        }

        if msg.op == MessageOp::Chat && self.scopes.contains(&msg.scope) {
            // Ignore our own messages (avoid self-trigger loops).
            if msg.origin == Origin::Head && msg.sender == self.head_id {
                return Trigger::None;
            }

            // Do not trigger on system/hand chatter by default (goal_service notifications, tool output, etc).
            // But allow humans and other heads in the same scope.
            if msg.origin == Origin::System || msg.origin == Origin::Hand {
                return Trigger::None;
            }

            return Trigger::Message(TriggerContext {
                msg_id: msg.id,
                scope: msg.scope.clone(),
            });
        }

        Trigger::None
    }

    async fn think(&self, ctx: Option<TriggerContext>) {
        let Some(llm) = &self.llm else { return };

        let bundle_builder = HeadBundleBuilder::new(self.store.clone());
        let bundle_cfg = HeadBundleConfig::new(&self.head_id, self.scopes.clone());
        let mut messages = bundle_builder.build(&bundle_cfg);

        tracing::info!(head = %self.head_id, message_count = messages.len(), "head thinking");

        let default_scope = ctx
            .as_ref()
            .map(|c| c.scope.to_string())
            .or_else(|| self.scopes.first().map(|s| s.to_string()))
            .unwrap_or_else(|| "main".to_string());

        let reply_to = ctx.as_ref().map(|c| c.msg_id);
        let run_id = reply_to
            .map(|id| id.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let tools = head_tool_specs();
        let tool_choice = serde_json::json!("auto");
        let policy = RetryPolicy::default_llm();

        let mut should_sleep = false;

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

                    if let Some(r) = reply_to {
                        let msg = respond::chat(
                            &self.head_id,
                            Scope::from(default_scope.as_str()),
                            format!("(error) head llm failed after retries: {}", e.message),
                        )
                        .with_origin(Origin::Head)
                        .with_reply_to(r);
                        self.bus.publish(msg).await;
                    }

                    should_sleep = true;
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
                        tool = %tc.function.name,
                        args = %tc.function.arguments,
                        "head tool call"
                    );
                }
            }
            if let Some(ref content) = result.content {
                if !content.trim().is_empty() {
                    tracing::info!(head = %self.head_id, content = %content, "head response");
                }
            }

            if !result.tool_calls.is_empty() {
                messages.push(crate::llm::ChatMessage::assistant_tool_calls(
                    result.tool_calls.clone(),
                ));

                for tc in &result.tool_calls {
                    let out = exec_head_tool(
                        &self.bus,
                        self.store.as_ref(),
                        &self.head_id,
                        &default_scope,
                        reply_to,
                        self.memory.as_ref(),
                        &tc.function.name,
                        &tc.function.arguments,
                    )
                    .await;
                    messages.push(crate::llm::ChatMessage::tool_result(tc.id.clone(), out));
                }
                continue;
            }

            let content = result.content.unwrap_or_default();
            if !content.trim().is_empty() {
                let mut chat = respond::chat(&self.head_id, Scope::from(default_scope.as_str()), content)
                    .with_origin(Origin::Head);
                if let Some(r) = reply_to {
                    chat = chat.with_reply_to(r);
                }
                self.bus.publish(chat).await;
            }

            should_sleep = true;
            break;
        }

        if should_sleep {
            // Signal the harness that the head is ready to sleep.
            // The harness emits per-chain Done when all tasks for that chain are drained.
            self.bus
                .publish(
                    respond::sleep(&self.head_id, Scope::head_mail(&self.head_id), 300)
                        .with_origin(Origin::Head),
                )
                .await;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Trigger {
    None,
    Heartbeat,
    Message(TriggerContext),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TriggerContext {
    msg_id: Uuid,
    scope: Scope,
}
