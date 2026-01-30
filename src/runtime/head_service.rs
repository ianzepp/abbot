use std::sync::Arc;
use std::time::Instant;

use tokio::time::timeout;
use uuid::Uuid;

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope, respond};
use crate::history::Store;
use crate::llm::OpenAICompatClient;

use super::head_parser::{parse_head_response, ChatAction, HandAction, HandCommand, MailAction};
use super::{HeadBundleBuilder, HeadBundleConfig, HeadConfig, RuntimeBus};

pub struct HeadService {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,
    head_cfg: HeadConfig,
    llm: Option<Arc<OpenAICompatClient>>,
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

                if trigger != Trigger::None && self.llm.is_some() {
                    pending_think = true;
                }
            }

            let should_think = pending_think
                && last_think.elapsed() >= self.head_cfg.debounce_interval;

            if should_think {
                tracing::info!(head = %self.head_id, "head thinking...");
                self.think().await;
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

        if msg.op == MessageOp::Chat && msg.origin == Origin::Human {
            if self.scopes.contains(&msg.scope) {
                return Trigger::HumanMessage;
            }
        }

        if msg.op == MessageOp::Chat && msg.origin == Origin::System {
            let head_scope = Scope::from(format!("@{}", self.head_id).as_str());
            if msg.scope == head_scope {
                return Trigger::SystemNotification;
            }
        }

        Trigger::None
    }

    async fn think(&self) {
        let Some(llm) = &self.llm else { return };

        let bundle_builder = HeadBundleBuilder::new(self.store.clone());
        let bundle_cfg = HeadBundleConfig::new(&self.head_id, self.scopes.clone());
        let messages = bundle_builder.build(&bundle_cfg);

        tracing::info!(head = %self.head_id, message_count = messages.len(), "head thinking");

        let result = match timeout(
            std::time::Duration::from_secs(120),
            llm.chat(messages),
        )
        .await
        {
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

        let default_scope = self.scopes.first().map(|s| s.to_string()).unwrap_or_else(|| "#general".to_string());
        let parsed = parse_head_response(&result.content, &default_scope);

        if parsed.is_empty() {
            tracing::info!(head = %self.head_id, "head produced no actions");
            return;
        }

        for action in &parsed.hands {
            self.execute_hand(action).await;
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
            .publish(
                respond::chat(&self.head_id, scope, &action.content)
                    .with_origin(Origin::Head),
            )
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
            .publish(
                respond::chat(&self.head_id, scope, &action.content)
                    .with_origin(Origin::Head),
            )
            .await;

        tracing::debug!(
            head = %self.head_id,
            recipient = %action.recipient,
            "head mail"
        );
    }

    async fn execute_hand(&self, action: &HandAction) {
        for cmd in &action.commands {
            match cmd {
                HandCommand::Goal(goal) => {
                    self.submit_goal(goal).await;
                }
                HandCommand::List | HandCommand::Read(_) | HandCommand::Clear(_) => {
                    tracing::debug!(
                        head = %self.head_id,
                        cmd = ?cmd,
                        "hand command ignored (managed by GoalService)"
                    );
                }
            }
        }
    }

    async fn submit_goal(&self, goal: &str) {
        let task_id = Uuid::new_v4().to_string();
        let scope = Scope::Task(format!("task/{}", task_id));

        self.bus.create_scope(scope.clone()).await;

        self.bus
            .publish(
                respond::task_request(
                    &self.head_id,
                    scope,
                    &task_id,
                    &self.head_id,
                    goal,
                    goal,
                )
                .with_origin(Origin::Head),
            )
            .await;

        tracing::info!(
            head = %self.head_id,
            task_id = %task_id,
            goal = %goal,
            "goal submitted to queue"
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    None,
    Heartbeat,
    HumanMessage,
    SystemNotification,
}
