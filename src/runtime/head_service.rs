use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope, TaskMsg, respond};
use crate::history::Store;
use crate::llm::OpenAICompatClient;

use super::head_parser::{parse_head_response, ChatAction, MailAction, TaskAction};
use super::{HeadBundleBuilder, HeadBundleConfig, HeadConfig, RuntimeBus};

struct TaskMeta {
    completed: bool,
}

pub struct HeadService {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,
    tasks: Arc<Mutex<HashMap<String, TaskMeta>>>,
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
        let head_cfg = HeadConfig::from_env();

        let llm = if head_cfg.enabled {
            tracing::info!(
                base_url = %head_cfg.base_url,
                model = %head_cfg.model,
                api_key_set = !head_cfg.api_key.is_empty(),
                temperature = ?head_cfg.temperature,
                max_tokens = ?head_cfg.max_tokens,
                heartbeat_tick = head_cfg.heartbeat_tick,
                "head llm enabled via HEAD_* env"
            );
            Some(Arc::new(OpenAICompatClient::new(
                &head_cfg.base_url,
                &head_cfg.api_key,
                &head_cfg.model,
                head_cfg.temperature,
                head_cfg.max_tokens,
                head_cfg.extra_headers.clone(),
            )))
        } else {
            None
        };

        Self {
            bus,
            store,
            head_id: head_id.into(),
            scopes,
            tasks: Arc::new(Mutex::new(HashMap::new())),
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
            // Use timeout to ensure we check pending_think even if no messages arrive
            let recv_timeout = if pending_think {
                self.head_cfg.debounce_interval
            } else {
                std::time::Duration::from_secs(60)
            };

            let msg = match timeout(recv_timeout, rx.recv()).await {
                Ok(Ok(m)) => Some(m),
                Ok(Err(_)) => continue,
                Err(_) => None, // timeout - check pending_think
            };

            if let Some(msg) = msg {
                let trigger = self.handle_message(&msg).await;

                if trigger != Trigger::None && self.llm.is_some() {
                    pending_think = true;
                }
            }

            // Debounce: batch rapid triggers
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
        // Heartbeat trigger (every N ticks)
        if msg.op == MessageOp::Ping {
            if let MessageData::Ping { tick, .. } = &msg.data {
                if self.head_cfg.heartbeat_tick > 0 && tick % self.head_cfg.heartbeat_tick == 0 {
                    return Trigger::Heartbeat;
                }
            }
            return Trigger::None;
        }

        // Track task metadata and handle completion
        if msg.op == MessageOp::Task {
            if let MessageData::Task(TaskMsg::Request { task_id, .. }) = &msg.data {
                let mut tasks = self.tasks.lock().await;
                tasks.entry(task_id.clone()).or_insert(TaskMeta { completed: false });
            }

            if let MessageData::Task(TaskMsg::Result { task_id, hand_id, ok, summary }) = &msg.data {
                let mut tasks = self.tasks.lock().await;
                if let Some(meta) = tasks.get_mut(task_id) {
                    if !meta.completed {
                        meta.completed = true;

                        // If LLM disabled, report result directly to first scope
                        if self.llm.is_none() {
                            if let Some(scope) = self.scopes.first() {
                                let status = if *ok { "OK" } else { "FAILED" };
                                let text = format!(
                                    "[task {}] {} (hand {})\n{}",
                                    task_id, status, hand_id, summary.trim()
                                );
                                self.bus
                                    .publish(
                                        respond::chat(&self.head_id, scope.clone(), text)
                                            .with_origin(Origin::Head),
                                    )
                                    .await;
                            }
                        }

                        return Trigger::TaskComplete;
                    }
                }
            }
        }

        // Trigger on human chat message in watched scopes
        if msg.op == MessageOp::Chat && msg.origin == Origin::Human {
            tracing::debug!(
                head = %self.head_id,
                scope = %msg.scope,
                watching = ?self.scopes,
                "human chat received"
            );
            if self.scopes.contains(&msg.scope) {
                return Trigger::HumanMessage;
            }
        }

        Trigger::None
    }

    async fn think(&self) {
        let Some(llm) = &self.llm else { return };

        let bundle_builder = HeadBundleBuilder::new(self.store.clone());
        let bundle_cfg = HeadBundleConfig::new(&self.head_id, self.scopes.clone());
        let messages = bundle_builder.build(&bundle_cfg);

        tracing::debug!(head = %self.head_id, message_count = messages.len(), "head thinking");

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

        let parsed = parse_head_response(&result.content);

        if parsed.is_empty() {
            tracing::debug!(head = %self.head_id, "head produced no actions");
            return;
        }

        // Execute actions
        for action in &parsed.chats {
            self.execute_chat(action).await;
        }

        for action in &parsed.mails {
            self.execute_mail(action).await;
        }

        for action in &parsed.tasks {
            self.execute_task(action).await;
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

    async fn execute_task(&self, action: &TaskAction) {
        let scope = Scope::Task(format!("task/{}", action.id));

        self.bus.create_scope(scope.clone()).await;

        self.bus
            .publish(
                respond::task_request(
                    &self.head_id,
                    scope,
                    &action.id,
                    &self.head_id,
                    &action.goal,
                    &action.content,
                )
                .with_origin(Origin::Head),
            )
            .await;

        tracing::debug!(
            head = %self.head_id,
            task_id = %action.id,
            goal = %action.goal,
            "head task created"
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    None,
    Heartbeat,
    TaskComplete,
    HumanMessage,
}
