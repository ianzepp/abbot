// GoalService manages the task queue and assigns work to available hands.
//
// Tasks flow from heads (which create goals) through this service to hands
// (which execute). The service maintains per-scope queues and a pool of hands,
// assigning work in FIFO order. It monitors hand health and reassigns tasks
// if hands become unresponsive. Timeout handling prevents stuck tasks.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::Mutex;
use uuid::Uuid;

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
    pub reply_to: Option<Uuid>,
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
    // notify_scope (e.g. "#general", "@alice") -> FIFO queue of goals for that scope
    queues: Arc<Mutex<HashMap<String, VecDeque<Goal>>>>,
    // Round-robin order of active notify scopes (only those with non-empty queues)
    rr_scopes: Arc<Mutex<VecDeque<String>>>,
    hands: Arc<Mutex<Vec<HandInfo>>>,
    active_goals: Arc<Mutex<HashMap<String, Goal>>>, // goal_id -> goal
    outstanding_by_notify_scope: Arc<Mutex<HashMap<String, usize>>>, // notify_scope -> count
    pool_size: usize,
    timeout_secs: u64,
}

impl GoalService {
    pub fn new(bus: RuntimeBus) -> Self {
        let config = AppConfig::global();
        let pool_size = config.pool.size.unwrap_or(DEFAULT_HAND_POOL_SIZE);
        let timeout_secs = config
            .pool
            .timeout_secs
            .unwrap_or(DEFAULT_GOAL_TIMEOUT_SECS);

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
            rr_scopes: Arc::new(Mutex::new(VecDeque::new())),
            hands: Arc::new(Mutex::new(hands)),
            active_goals: Arc::new(Mutex::new(HashMap::new())),
            outstanding_by_notify_scope: Arc::new(Mutex::new(HashMap::new())),
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
                    self.handle_task_msg(msg.scope.clone(), msg.reply_to, task_msg.clone())
                        .await;
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

    async fn handle_task_msg(&self, scope: Scope, reply_to: Option<Uuid>, task_msg: TaskMsg) {
        match task_msg {
            TaskMsg::Request {
                task_id,
                head_id,
                goal,
                input,
                notify_scope,
            } => {
                self.enqueue_goal(scope, reply_to, task_id, head_id, goal, input, notify_scope)
                    .await;
            }
            TaskMsg::Result {
                task_id,
                hand_id,
                ok,
                summary,
            } => {
                self.handle_result(task_id, hand_id, ok, summary).await;
            }
            _ => {}
        }
    }

    async fn enqueue_goal(
        &self,
        scope: Scope,
        reply_to: Option<Uuid>,
        task_id: String,
        head_id: String,
        goal_text: String,
        input: String,
        notify_scope: Option<String>,
    ) {
        let notify_scope_key = notify_scope.as_deref().unwrap_or("#general").to_string();

        let goal = Goal {
            id: task_id.clone(),
            head_id: head_id.clone(),
            scope: scope.clone(),
            goal: goal_text.clone(),
            input,
            notify_scope: notify_scope.clone(),
            reply_to,
            created_at: Instant::now(),
        };

        self.increment_outstanding(&notify_scope_key).await;

        {
            let mut rr = self.rr_scopes.lock().await;
            let mut queues = self.queues.lock().await;

            let existed = queues.contains_key(&notify_scope_key);
            queues
                .entry(notify_scope_key.clone())
                .or_default()
                .push_back(goal.clone());
            if !existed {
                rr.push_back(notify_scope_key.clone());
            }
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
            hands
                .iter()
                .find(|h| h.state == HandState::Idle)
                .map(|h| h.hand_id.clone())
        };

        let Some(hand_id) = idle_hand else { return };

        let next_goal = {
            let mut rr = self.rr_scopes.lock().await;
            let mut queues = self.queues.lock().await;

            let mut goal: Option<Goal> = None;
            let mut remaining = rr.len();
            while remaining > 0 {
                remaining -= 1;
                let Some(scope_key) = rr.pop_front() else {
                    break;
                };

                let Some(queue) = queues.get_mut(&scope_key) else {
                    continue;
                };

                if let Some(g) = queue.pop_front() {
                    if queue.is_empty() {
                        queues.remove(&scope_key);
                    } else {
                        rr.push_back(scope_key);
                    }
                    goal = Some(g);
                    break;
                }

                // Queue is empty (should be rare): drop it.
                queues.remove(&scope_key);
            }

            goal
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

        if let Some(goal) = goal {
            tracing::info!(
                task_id = %task_id,
                hand_id = %hand_id,
                ok = ok,
                notify_scope = ?goal.notify_scope,
                "goal {}", status
            );
            let msg = if ok {
                format!(
                    "Goal {}: {}\nResult: {}",
                    status,
                    truncate(&goal.goal, 40),
                    truncate(&summary, 100)
                )
            } else {
                format!(
                    "Goal {}: {}\nError: {}",
                    status,
                    truncate(&goal.goal, 40),
                    truncate(&summary, 100)
                )
            };

            let notify_scope_key = goal
                .notify_scope
                .as_deref()
                .unwrap_or("#general")
                .to_string();

            let drained = self.decrement_outstanding(&notify_scope_key).await;
            self.notify(&goal.notify_scope, goal.reply_to, &msg).await;
            if drained {
                self.notify_drained(&notify_scope_key, goal.reply_to).await;
            }
        } else {
            tracing::warn!(
                task_id = %task_id,
                hand_id = %hand_id,
                ok = ok,
                "goal {} but goal not found in active_goals", status
            );
        }
    }

    async fn check_timeouts(&self) {
        let now = Instant::now();
        let timeout = Duration::from_secs(self.timeout_secs);

        let timed_out: Vec<(String, String, Option<String>, Option<Uuid>)> = {
            let hands = self.hands.lock().await;
            let active = self.active_goals.lock().await;
            hands
                .iter()
                .filter_map(|h| {
                    if let HandState::Running {
                        task_id,
                        started_at,
                        ..
                    } = &h.state
                    {
                        if now.duration_since(*started_at) > timeout {
                            let (notify_scope, reply_to) = active
                                .get(task_id)
                                .map(|g| (g.notify_scope.clone(), g.reply_to))
                                .unwrap_or((None, None));
                            return Some((
                                h.hand_id.clone(),
                                task_id.clone(),
                                notify_scope,
                                reply_to,
                            ));
                        }
                    }
                    None
                })
                .collect()
        };

        for (hand_id, task_id, notify_scope, reply_to) in timed_out {
            tracing::warn!(
                task_id = %task_id,
                hand_id = %hand_id,
                timeout_secs = self.timeout_secs,
                "goal timed out"
            );

            {
                let mut hands = self.hands.lock().await;
                if let Some(hand) = hands.iter_mut().find(|h| h.hand_id == hand_id) {
                    hand.state = HandState::Idle;
                }
            }

            {
                let mut active = self.active_goals.lock().await;
                if active.remove(&task_id).is_none() {
                    tracing::warn!(
                        task_id = %task_id,
                        hand_id = %hand_id,
                        "goal timed out but goal not found in active_goals"
                    );
                }
            }

            // Best-effort cleanup in case the goal was still present in a queue.
            {
                let notify_scope_key = notify_scope.as_deref().unwrap_or("#general").to_string();
                let mut rr = self.rr_scopes.lock().await;
                let mut queues = self.queues.lock().await;

                if let Some(queue) = queues.get_mut(&notify_scope_key) {
                    if let Some(pos) = queue.iter().position(|g| g.id == task_id) {
                        queue.remove(pos);
                    }
                    if queue.is_empty() {
                        queues.remove(&notify_scope_key);
                        if let Some(pos) = rr.iter().position(|s| s == &notify_scope_key) {
                            rr.remove(pos);
                        }
                    }
                }
            }

            let task_short = task_id.chars().take(8).collect::<String>();
            let notify_scope_key = notify_scope.as_deref().unwrap_or("#general").to_string();
            let drained = self.decrement_outstanding(&notify_scope_key).await;
            self.notify(
                &notify_scope,
                reply_to,
                &format!(
                    "Goal timed out ({}s): task {}",
                    self.timeout_secs, task_short
                ),
            )
            .await;
            if drained {
                self.notify_drained(&notify_scope_key, reply_to).await;
            }
        }
    }

    async fn increment_outstanding(&self, notify_scope: &str) {
        let mut map = self.outstanding_by_notify_scope.lock().await;
        *map.entry(notify_scope.to_string()).or_insert(0) += 1;
    }

    async fn decrement_outstanding(&self, notify_scope: &str) -> bool {
        let mut map = self.outstanding_by_notify_scope.lock().await;
        let Some(v) = map.get_mut(notify_scope) else {
            return false;
        };

        if *v == 0 {
            return false;
        }

        *v -= 1;
        if *v == 0 {
            map.remove(notify_scope);
            return true;
        }

        false
    }

    async fn notify_drained(&self, notify_scope: &str, reply_to: Option<Uuid>) {
        let scope = Scope::from(notify_scope);
        let payload = json!({ "scope": notify_scope });
        let mut msg = respond::event("goal_service", scope, "goals_drained", payload)
            .with_origin(Origin::System);
        if let Some(reply_to) = reply_to {
            msg = msg.with_reply_to(reply_to);
        }
        self.bus.publish(msg).await;
    }

    async fn notify(&self, notify_scope: &Option<String>, reply_to: Option<Uuid>, message: &str) {
        let scope_str = notify_scope.as_deref().unwrap_or("#general");
        let scope = Scope::from(scope_str);
        let mut msg = respond::chat("goal_service", scope, message).with_origin(Origin::System);
        if let Some(reply_to) = reply_to {
            msg = msg.with_reply_to(reply_to);
        }
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
        let mut emptied_scope: Option<String> = None;

        {
            let mut rr = self.rr_scopes.lock().await;
            let mut queues = self.queues.lock().await;
            for (scope_key, queue) in queues.iter_mut() {
                if let Some(pos) = queue.iter().position(|g| g.id == task_id) {
                    queue.remove(pos);
                    if queue.is_empty() {
                        emptied_scope = Some(scope_key.clone());
                    }
                    found = true;
                    break;
                }
            }

            if let Some(scope_key) = &emptied_scope {
                queues.remove(scope_key);
                if let Some(pos) = rr.iter().position(|s| s == scope_key) {
                    rr.remove(pos);
                }
            }
        }

        if found {
            let goal = {
                let mut active = self.active_goals.lock().await;
                active.remove(task_id)
            };
            tracing::info!(task_id = %task_id, "goal cancelled from queue");

            if let Some(goal) = goal {
                let notify_scope_key = goal
                    .notify_scope
                    .as_deref()
                    .unwrap_or("#general")
                    .to_string();
                let drained = self.decrement_outstanding(&notify_scope_key).await;
                if drained {
                    self.notify_drained(&notify_scope_key, goal.reply_to).await;
                }
            }
        }

        found
    }
}

fn truncate(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        return s.to_string();
    }

    let clipped = s.chars().take(max).collect::<String>();
    format!("{}...", clipped)
}
