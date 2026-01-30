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

use super::head_parser::{ChatAction, GoalAction, MailAction, parse_head_response};
use super::{HeadBundleBuilder, HeadBundleConfig, HeadConfig, RuntimeBus};

pub struct HeadService {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,
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

                if let Trigger::Message(ctx) = &trigger {
                    *self.pending_ctx.lock().await = Some(ctx.clone());
                }

                if trigger != Trigger::None && self.llm.is_some() {
                    pending_think = true;
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
        if msg.op == MessageOp::Ping {
            if let MessageData::Ping { tick, .. } = &msg.data {
                if self.head_cfg.heartbeat_tick > 0 && tick % self.head_cfg.heartbeat_tick == 0 {
                    return Trigger::Heartbeat;
                }
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
                return Trigger::Message(TriggerContext {
                    msg_id: msg.id,
                    scope: msg.scope.clone(),
                });
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
        let messages = bundle_builder.build(&bundle_cfg);

        tracing::info!(head = %self.head_id, message_count = messages.len(), "head thinking");

        let result = match timeout(std::time::Duration::from_secs(120), llm.chat(messages)).await {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => {
                tracing::error!(head = %self.head_id, error = %e, "head llm error");
                return;
            }
            Err(_) => {
                tracing::error!(head = %self.head_id, "head llm timeout");
                return;
            }
        };

        tracing::info!(head = %self.head_id, "\n--- HEAD RESPONSE ---\n{}\n--- END RESPONSE ---", result.content);

        let default_scope = ctx
            .as_ref()
            .map(|c| c.scope.to_string())
            .or_else(|| self.scopes.first().map(|s| s.to_string()))
            .unwrap_or_else(|| "#general".to_string());
        let parsed = parse_head_response(&result.content, &default_scope);

        if parsed.is_empty() {
            tracing::info!(head = %self.head_id, "head produced no actions");
            return;
        }

        for action in &parsed.goals {
            self.execute_goal(action, &ctx).await;
        }

        for action in &parsed.chats {
            self.execute_chat(action).await;
        }

        for action in &parsed.mails {
            self.execute_mail(action).await;
        }
    }

    async fn execute_chat(&self, action: &ChatAction) {
        let scope = Scope::from(action.scope.as_str());

        self.bus
            .publish(respond::chat(&self.head_id, scope, &action.content).with_origin(Origin::Head))
            .await;

        tracing::debug!(
            head = %self.head_id,
            scope = %action.scope,
            "head chat"
        );
    }

    async fn execute_mail(&self, action: &MailAction) {
        let scope = Scope::from(action.recipient.as_str());

        self.bus
            .publish(respond::chat(&self.head_id, scope, &action.content).with_origin(Origin::Head))
            .await;

        tracing::debug!(
            head = %self.head_id,
            recipient = %action.recipient,
            "head mail"
        );
    }

    async fn execute_goal(&self, action: &GoalAction, ctx: &Option<TriggerContext>) {
        let task_id = Uuid::new_v4().to_string();
        let scope = Scope::Task(format!("task/{}", task_id));
        let notify_scope = ctx
            .as_ref()
            .map(|c| c.scope.to_string())
            .or_else(|| self.scopes.first().map(|s| s.to_string()))
            .unwrap_or_else(|| "#general".to_string());

        self.bus.create_scope(scope.clone()).await;

        let mut req = respond::task_request_with_notify(
            &self.head_id,
            scope,
            &task_id,
            &self.head_id,
            &action.goal,
            &action.goal,
            &notify_scope,
        )
        .with_origin(Origin::Head);

        if let Some(ctx) = ctx {
            req = req.with_reply_to(ctx.msg_id);
        }

        self.bus.publish(req).await;

        tracing::info!(
            head = %self.head_id,
            task_id = %task_id,
            goal = %action.goal,
            notify_scope = %notify_scope,
            "goal submitted to queue"
        );
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
