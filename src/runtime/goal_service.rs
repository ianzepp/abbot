use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;


use crate::bus::{MessageData, MessageOp, Origin, Scope, TaskMsg, respond};

use super::{AppConfig, RuntimeBus};

const DEFAULT_HAND_POOL_SIZE: usize = 4;
const DEFAULT_GOAL_TIMEOUT_SECS: u64 = 300; // 5 minutes
const DISPATCH_INTERVAL_MS: u64 = 100;

#[derive(Clone, Debug)]
pub struct Goal {
    pub id: String,
    pub head_id: String,
    pub scope: Scope,
    pub goal: String,
    pub input: String,
    pub notify_scope: Option<String>,
    pub created_at: Instant,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HandState {
    Idle,
    Running {
        task_id: String,
        goal_id: String,
        head_id: String,
        started_at: Instant,
    },
}

#[derive(Clone, Debug)]
pub struct HandInfo {
    pub hand_id: String,
    pub state: HandState,
}

pub struct GoalService {
    bus: RuntimeBus,
    queues: Arc<Mutex<HashMap<String, VecDeque<Goal>>>>, // scope -> queue
    hands: Arc<Mutex<Vec<HandInfo>>>,
    active_goals: Arc<Mutex<HashMap<String, Goal>>>, // goal_id -> goal
    pool_size: usize,
    timeout_secs: u64,
}

impl GoalService {
    pub fn new(bus: RuntimeBus) -> Self {
        let config = AppConfig::global();
        let pool_size = config.pool.size.unwrap_or(DEFAULT_HAND_POOL_SIZE);
        let timeout_secs = config.pool.timeout_secs.unwrap_or(DEFAULT_GOAL_TIMEOUT_SECS);

        Self::with_config(bus, pool_size, timeout_secs)
    }

    pub fn with_config(bus: RuntimeBus, pool_size: usize, timeout_secs: u64) -> Self {
        let hands: Vec<HandInfo> = (0..pool_size)
            .map(|i| HandInfo {
                hand_id: format!("hand-{}", i),
                state: HandState::Idle,
            })
            .collect();

        tracing::info!(
            pool_size = pool_size,
            timeout_secs = timeout_secs,
            "goal service configured"
        );

        Self {
            bus,
            queues: Arc::new(Mutex::new(HashMap::new())),
            hands: Arc::new(Mutex::new(hands)),
            active_goals: Arc::new(Mutex::new(HashMap::new())),
            pool_size,
            timeout_secs,
        }
    }

    pub fn start(self: Arc<Self>) {
        let svc = self.clone();
        tokio::spawn(async move {
            svc.run_message_loop().await;
        });

        let svc = self.clone();
        tokio::spawn(async move {
            svc.run_dispatch_loop().await;
        });

        let svc = self.clone();
        tokio::spawn(async move {
            svc.run_timeout_loop().await;
        });
    }

    async fn run_message_loop(&self) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        tracing::info!(pool_size = self.pool_size, "goal service started");

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            match (&msg.op, &msg.data) {
                (MessageOp::Task, MessageData::Task(task_msg)) => {
                    self.handle_task_msg(msg.scope.clone(), task_msg.clone()).await;
                }
                _ => {}
            }
        }
    }

    async fn run_dispatch_loop(&self) {
        loop {
            self.try_dispatch().await;
            tokio::time::sleep(Duration::from_millis(DISPATCH_INTERVAL_MS)).await;
        }
    }

    async fn run_timeout_loop(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            self.check_timeouts().await;
        }
    }

    async fn handle_task_msg(&self, scope: Scope, task_msg: TaskMsg) {
        match task_msg {
            TaskMsg::Request { task_id, head_id, goal, input, notify_scope } => {
                self.enqueue_goal(scope, task_id, head_id, goal, input, notify_scope).await;
            }
            TaskMsg::Result { task_id, hand_id, ok, summary } => {
                self.handle_result(task_id, hand_id, ok, summary).await;
            }
            _ => {}
        }
    }

    async fn enqueue_goal(&self, scope: Scope, task_id: String, head_id: String, goal_text: String, input: String, notify_scope: Option<String>) {
        let goal = Goal {
            id: task_id.clone(),
            head_id: head_id.clone(),
            scope: scope.clone(),
            goal: goal_text.clone(),
            input,
            notify_scope: notify_scope.clone(),
            created_at: Instant::now(),
        };

        let scope_key = scope.to_string();

        {
            let mut queues = self.queues.lock().await;
            queues.entry(scope_key.clone()).or_default().push_back(goal.clone());
        }

        {
            let mut active = self.active_goals.lock().await;
            active.insert(task_id.clone(), goal);
        }

        tracing::info!(
            task_id = %task_id,
            head_id = %head_id,
            goal = %goal_text,
            notify_scope = ?notify_scope,
            "goal queued"
        );
    }

    async fn try_dispatch(&self) {
        let idle_hand = {
            let hands = self.hands.lock().await;
            hands.iter().find(|h| h.state == HandState::Idle).map(|h| h.hand_id.clone())
        };

        let Some(hand_id) = idle_hand else { return };

        let next_goal = {
            let mut queues = self.queues.lock().await;
            let mut found = None;
            for (_scope, queue) in queues.iter_mut() {
                if let Some(goal) = queue.pop_front() {
                    found = Some(goal);
                    break;
                }
            }
            found
        };

        let Some(goal) = next_goal else { return };

        {
            let mut hands = self.hands.lock().await;
            if let Some(hand) = hands.iter_mut().find(|h| h.hand_id == hand_id) {
                hand.state = HandState::Running {
                    task_id: goal.id.clone(),
                    goal_id: goal.id.clone(),
                    head_id: goal.head_id.clone(),
                    started_at: Instant::now(),
                };
            }
        }

        tracing::info!(
            task_id = %goal.id,
            hand_id = %hand_id,
            head_id = %goal.head_id,
            goal = %goal.goal,
            "dispatching goal to hand"
        );

        let assigned_msg = respond::task_assigned(
            "goal_service",
            goal.scope.clone(),
            goal.id.clone(),
            goal.head_id.clone(),
            hand_id.clone(),
        )
        .with_origin(Origin::System);

        self.bus.publish(assigned_msg).await;
    }

    async fn handle_result(&self, task_id: String, hand_id: String, ok: bool, summary: String) {
        let goal = {
            let mut active = self.active_goals.lock().await;
            active.remove(&task_id)
        };

        {
            let mut hands = self.hands.lock().await;
            if let Some(hand) = hands.iter_mut().find(|h| h.hand_id == hand_id) {
                hand.state = HandState::Idle;
            }
        }

        let status = if ok { "completed" } else { "failed" };
        tracing::info!(
            task_id = %task_id,
            hand_id = %hand_id,
            ok = ok,
            "goal {}", status
        );

        if let Some(goal) = goal {
            let msg = if ok {
                format!("Goal {}: {}\nResult: {}", status, truncate(&goal.goal, 40), truncate(&summary, 100))
            } else {
                format!("Goal {}: {}\nError: {}", status, truncate(&goal.goal, 40), truncate(&summary, 100))
            };
            self.notify(&goal.notify_scope, &msg).await;
        }
    }

    async fn check_timeouts(&self) {
        let now = Instant::now();
        let timeout = Duration::from_secs(self.timeout_secs);

        let timed_out: Vec<(String, Option<String>)> = {
            let hands = self.hands.lock().await;
            let active = self.active_goals.lock().await;
            hands
                .iter()
                .filter_map(|h| {
                    if let HandState::Running { task_id, started_at, .. } = &h.state {
                        if now.duration_since(*started_at) > timeout {
                            let notify_scope = active.get(task_id).and_then(|g| g.notify_scope.clone());
                            return Some((task_id.clone(), notify_scope));
                        }
                    }
                    None
                })
                .collect()
        };

        for (task_id, notify_scope) in timed_out {
            tracing::warn!(
                task_id = %task_id,
                timeout_secs = self.timeout_secs,
                "goal timed out"
            );

            {
                let mut hands = self.hands.lock().await;
                if let Some(hand) = hands.iter_mut().find(|h| {
                    matches!(&h.state, HandState::Running { task_id: tid, .. } if tid == &task_id)
                }) {
                    hand.state = HandState::Idle;
                }
            }

            {
                let mut active = self.active_goals.lock().await;
                active.remove(&task_id);
            }

            self.notify(&notify_scope, &format!("Goal timed out ({}s): task {}", self.timeout_secs, &task_id[..8])).await;
        }
    }

    async fn notify(&self, notify_scope: &Option<String>, message: &str) {
        let scope_str = notify_scope.as_deref().unwrap_or("#general");
        let scope = Scope::from(scope_str);
        let msg = respond::chat("goal_service", scope, message).with_origin(Origin::System);
        self.bus.publish(msg).await;
    }

    pub async fn queue_status(&self) -> HashMap<String, usize> {
        let queues = self.queues.lock().await;
        queues.iter().map(|(k, v)| (k.clone(), v.len())).collect()
    }

    pub async fn hand_status(&self) -> Vec<HandInfo> {
        self.hands.lock().await.clone()
    }

    pub async fn cancel_goal(&self, task_id: &str) -> bool {
        let mut found = false;

        {
            let mut queues = self.queues.lock().await;
            for queue in queues.values_mut() {
                if let Some(pos) = queue.iter().position(|g| g.id == task_id) {
                    queue.remove(pos);
                    found = true;
                    break;
                }
            }
        }

        if found {
            let mut active = self.active_goals.lock().await;
            active.remove(task_id);
            tracing::info!(task_id = %task_id, "goal cancelled from queue");
        }

        found
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
