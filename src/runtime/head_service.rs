use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Mutex;
use tokio::time::timeout;
use uuid::Uuid;

use crate::bus::{Message, MessageData, MessageOp, Origin, Scope, TaskMsg, respond};
use crate::history::Store;
use crate::llm::OpenAICompatClient;

use super::head_parser::{parse_head_response, ChatAction, HandAction, HandCommand, MailAction};
use super::{HeadBundleBuilder, HeadBundleConfig, HeadConfig, RuntimeBus};

const NUM_HAND_SLOTS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandState {
    Idle,
    Running,
    Success,
    Failed,
}

#[derive(Debug, Clone)]
pub struct HandSlot {
    pub index: usize,
    pub state: HandState,
    pub task_id: Option<String>,
    pub goal: Option<String>,
    pub result: Option<String>,
}

impl HandSlot {
    fn new(index: usize) -> Self {
        Self {
            index,
            state: HandState::Idle,
            task_id: None,
            goal: None,
            result: None,
        }
    }

    fn hand_id(&self, head_id: &str) -> String {
        format!("{}/hand-{}", head_id, self.index)
    }

    fn assign(&mut self, task_id: String, goal: String) {
        self.state = HandState::Running;
        self.task_id = Some(task_id);
        self.goal = Some(goal);
        self.result = None;
    }

    fn complete(&mut self, ok: bool, summary: String) {
        self.state = if ok { HandState::Success } else { HandState::Failed };
        self.result = Some(summary);
    }

    fn reset(&mut self) {
        self.state = HandState::Idle;
        self.task_id = None;
        self.goal = None;
        self.result = None;
    }

    fn is_available(&self) -> bool {
        self.state == HandState::Idle
    }

    fn needs_acknowledgment(&self) -> bool {
        matches!(self.state, HandState::Success | HandState::Failed)
    }
}

pub struct HeadService {
    bus: RuntimeBus,
    store: Arc<Store>,
    head_id: String,
    scopes: Vec<Scope>,
    hands: Arc<Mutex<Vec<HandSlot>>>,
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
                num_hands = NUM_HAND_SLOTS,
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

        let hands: Vec<HandSlot> = (0..NUM_HAND_SLOTS).map(HandSlot::new).collect();

        Self {
            bus,
            store,
            head_id,
            scopes,
            hands: Arc::new(Mutex::new(hands)),
            head_cfg,
            llm,
        }
    }

    pub async fn hand_slots(&self) -> Vec<HandSlot> {
        self.hands.lock().await.clone()
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

        // Track task completion in hand slots
        if msg.op == MessageOp::Task {
            if let MessageData::Task(TaskMsg::Result { task_id, hand_id, ok, summary }) = &msg.data {
                let completion_info = {
                    let mut hands = self.hands.lock().await;
                    
                    // Find the hand slot for this task
                    if let Some(slot) = hands.iter_mut().find(|h| {
                        h.task_id.as_ref() == Some(task_id) && h.hand_id(&self.head_id) == *hand_id
                    }) {
                        if slot.state == HandState::Running {
                            let goal = slot.goal.clone().unwrap_or_default();
                            let slot_index = slot.index;
                            slot.complete(*ok, summary.clone());
                            
                            tracing::info!(
                                head = %self.head_id,
                                hand = slot_index,
                                task_id = %task_id,
                                ok = %ok,
                                "slot completed"
                            );

                            Some((slot_index, goal, *ok, summary.clone()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };

                if let Some((slot_index, goal, ok, summary)) = completion_info {
                    // Echo result to watched scope so head sees it in conversation
                    if let Some(notify_scope) = self.scopes.first() {
                        let status = if ok { "completed" } else { "failed" };
                        let text = format!(
                            "[hand-{} {} task: {}]\n{}",
                            slot_index, status, goal, summary.trim()
                        );
                        self.bus
                            .publish(
                                respond::chat(&format!("{}/hand-{}", self.head_id, slot_index), notify_scope.clone(), text)
                                    .with_origin(Origin::Hand),
                            )
                            .await;
                    }

                    return Trigger::TaskComplete;
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

        // Execute actions - hands first (so results are available for other actions)
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
        let mut output_lines = Vec::new();

        for cmd in &action.commands {
            match cmd {
                HandCommand::List => {
                    let hands = self.hands.lock().await;
                    for slot in hands.iter() {
                        let status = match slot.state {
                            HandState::Idle => "idle".to_string(),
                            HandState::Running => {
                                format!("running goal={:?}", slot.goal.as_deref().unwrap_or("?"))
                            }
                            HandState::Success => {
                                format!("success goal={:?}", slot.goal.as_deref().unwrap_or("?"))
                            }
                            HandState::Failed => {
                                format!("failed goal={:?}", slot.goal.as_deref().unwrap_or("?"))
                            }
                        };
                        output_lines.push(format!("hand-{}: {}", slot.index, status));
                    }
                }
                HandCommand::Goal(goal) => {
                    self.execute_goal(goal).await;
                }
                HandCommand::Read(index) => {
                    let hands = self.hands.lock().await;
                    if let Some(slot) = hands.get(*index) {
                        output_lines.push(format!("hand-{}: state={:?}", index, slot.state));
                        if let Some(task_id) = &slot.task_id {
                            output_lines.push(format!("  task_id: {}", task_id));
                        }
                        if let Some(goal) = &slot.goal {
                            output_lines.push(format!("  goal: {}", goal));
                        }
                        if let Some(result) = &slot.result {
                            output_lines.push(format!("  result: {}", result));
                        }
                    } else {
                        output_lines.push(format!("hand-{}: not found", index));
                    }
                }
                HandCommand::Clear(index) => {
                    let mut hands = self.hands.lock().await;
                    if let Some(slot) = hands.get_mut(*index) {
                        if slot.needs_acknowledgment() {
                            slot.reset();
                            output_lines.push(format!("hand-{}: cleared", index));
                            tracing::info!(
                                head = %self.head_id,
                                hand = index,
                                "slot cleared"
                            );
                        } else {
                            output_lines.push(format!("hand-{}: nothing to clear (state={:?})", index, slot.state));
                        }
                    } else {
                        output_lines.push(format!("hand-{}: not found", index));
                    }
                }
            }
        }

        // Publish hand command results to watched scope
        if !output_lines.is_empty() {
            if let Some(notify_scope) = self.scopes.first() {
                let text = format!("[hand status]\n{}", output_lines.join("\n"));
                self.bus
                    .publish(
                        respond::chat("_harness", notify_scope.clone(), text)
                            .with_origin(Origin::System),
                    )
                    .await;
            }
        }
    }

    async fn execute_goal(&self, goal: &str) {
        // Find an available hand slot
        let hand_slot = {
            let mut hands = self.hands.lock().await;
            if let Some(slot) = hands.iter_mut().find(|h| h.is_available()) {
                let task_id = Uuid::new_v4().to_string();
                slot.assign(task_id.clone(), goal.to_string());
                Some((slot.index, slot.hand_id(&self.head_id), task_id))
            } else {
                None
            }
        };

        let Some((slot_index, hand_id, task_id)) = hand_slot else {
            tracing::warn!(
                head = %self.head_id,
                goal = %goal,
                "no available hand slots, task dropped"
            );
            
            // Notify head via watched scope that the goal was dropped
            if let Some(notify_scope) = self.scopes.first() {
                let text = format!(
                    "[hand status]\ngoal dropped (no slots available): {}\nUse `list` to see slot states, `clear N` to free completed slots.",
                    goal
                );
                self.bus
                    .publish(
                        respond::chat("_harness", notify_scope.clone(), text)
                            .with_origin(Origin::System),
                    )
                    .await;
            }
            return;
        };

        let scope = Scope::Task(format!("task/{}", task_id));
        self.bus.create_scope(scope.clone()).await;

        // Publish task request
        self.bus
            .publish(
                respond::task_request(
                    &self.head_id,
                    scope.clone(),
                    &task_id,
                    &self.head_id,
                    goal,
                    goal, // input is same as goal (no multi-line body)
                )
                .with_origin(Origin::Head),
            )
            .await;

        // Publish task assignment (this triggers HandService)
        self.bus
            .publish(
                respond::task_assigned(
                    &self.head_id,
                    scope,
                    &task_id,
                    &self.head_id,
                    &hand_id,
                )
                .with_origin(Origin::System),
            )
            .await;

        tracing::info!(
            head = %self.head_id,
            hand = slot_index,
            task_id = %task_id,
            goal = %goal,
            "slot assigned"
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
