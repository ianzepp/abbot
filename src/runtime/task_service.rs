// TaskService manages the task queue and assigns work to available hands.
//
// Tasks flow from heads through this service to hands (which execute).
// The service maintains per-scope queues and a pool of hands,
// assigning work in FIFO order. It monitors hand health and reassigns tasks
// if hands become unresponsive. Timeout handling prevents stuck tasks.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::bus::{MessageData, MessageOp, Origin, Scope, TaskMsg, respond};

use super::proc_service::{ProcHandle, ProcKind};
use super::{AppConfig, RuntimeBus};

const DEFAULT_HAND_POOL_SIZE: usize = 4;
const DEFAULT_TASK_TIMEOUT_SECS: u64 = 300;

#[derive(Clone, Debug)]
pub struct Task {
    pub id: String,
    pub head_id: String,
    pub scope: Scope,
    pub goal: String,
    pub notify_scope: Option<String>,
    pub reply_to: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HandState {
    Idle,
    Running {
        task_id: String,
        head_id: String,
        started_at: Instant,
    },
}

#[derive(Clone, Debug)]
pub struct HandInfo {
    pub hand_id: String,
    pub state: HandState,
}

/// Read-only query handle for TaskService state.
/// Used by heads to introspect pending/running tasks.
#[derive(Clone)]
pub struct TaskServiceQuery {
    queues: Arc<Mutex<HashMap<String, VecDeque<Task>>>>,
    hands: Arc<Mutex<Vec<HandInfo>>>,
    active_tasks: Arc<Mutex<HashMap<String, Task>>>,
}

impl TaskServiceQuery {
    /// Get all pending (queued) tasks.
    pub async fn pending_tasks(&self) -> Vec<Task> {
        let queues = self.queues.lock().await;
        queues.values().flatten().cloned().collect()
    }

    /// Get hand status (which hands are idle/running).
    pub async fn hand_status(&self) -> Vec<HandInfo> {
        self.hands.lock().await.clone()
    }

    /// Get all active (currently running) tasks.
    pub async fn active_tasks(&self) -> Vec<Task> {
        self.active_tasks.lock().await.values().cloned().collect()
    }

    /// Get a specific task by ID (checks active first, then queued).
    pub async fn get_task(&self, task_id: &str) -> Option<Task> {
        // Check active tasks first
        {
            let active = self.active_tasks.lock().await;
            if let Some(task) = active.get(task_id) {
                return Some(task.clone());
            }
        }

        // Check queued tasks
        let queues = self.queues.lock().await;
        for queue in queues.values() {
            if let Some(task) = queue.iter().find(|t| t.id == task_id) {
                return Some(task.clone());
            }
        }

        None
    }
}

pub struct TaskService {
    bus: RuntimeBus,
    proc: ProcHandle,
    queues: Arc<Mutex<HashMap<String, VecDeque<Task>>>>,
    rr_scopes: Arc<Mutex<VecDeque<String>>>,
    hands: Arc<Mutex<Vec<HandInfo>>>,
    active_tasks: Arc<Mutex<HashMap<String, Task>>>,
    outstanding_by_head_notify: Arc<Mutex<HashMap<OutstandingKey, usize>>>,
    dispatch_notify: Arc<Notify>,
    pool_size: usize,
    timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OutstandingKey {
    head_id: String,
    notify_scope: String,
}

impl TaskService {
    pub fn new(bus: RuntimeBus, proc: ProcHandle) -> Self {
        let config = AppConfig::global();
        let pool_size = config.pool.size.unwrap_or(DEFAULT_HAND_POOL_SIZE);
        let timeout_secs = config
            .pool
            .timeout_secs
            .unwrap_or(DEFAULT_TASK_TIMEOUT_SECS);

        Self::with_config(bus, proc, pool_size, timeout_secs)
    }

    pub fn with_config(
        bus: RuntimeBus,
        proc: ProcHandle,
        pool_size: usize,
        timeout_secs: u64,
    ) -> Self {
        let hands: Vec<HandInfo> = (0..pool_size)
            .map(|i| HandInfo {
                hand_id: format!("hand-{}", i),
                state: HandState::Idle,
            })
            .collect();

        tracing::debug!(
            pool_size = pool_size,
            timeout_secs = timeout_secs,
            "task service configured"
        );

        Self {
            bus,
            proc,
            queues: Arc::new(Mutex::new(HashMap::new())),
            rr_scopes: Arc::new(Mutex::new(VecDeque::new())),
            hands: Arc::new(Mutex::new(hands)),
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
            outstanding_by_head_notify: Arc::new(Mutex::new(HashMap::new())),
            dispatch_notify: Arc::new(Notify::new()),
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
        tracing::debug!(pool_size = self.pool_size, "task service started");

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
            while self.try_dispatch().await {}
            self.dispatch_notify.notified().await;
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
                self.enqueue_task(scope, reply_to, task_id, head_id, goal, input, notify_scope)
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

    async fn enqueue_task(
        &self,
        scope: Scope,
        reply_to: Option<Uuid>,
        task_id: String,
        head_id: String,
        goal_text: String,
        _input: String,
        notify_scope: Option<String>,
    ) {
        let notify_scope_key = notify_scope.as_deref().unwrap_or("main").to_string();

        let task = Task {
            id: task_id.clone(),
            head_id: head_id.clone(),
            scope: scope.clone(),
            goal: goal_text.clone(),
            notify_scope: notify_scope.clone(),
            reply_to,
        };

        self.increment_outstanding(&head_id, &notify_scope_key)
            .await;

        {
            let mut rr = self.rr_scopes.lock().await;
            let mut queues = self.queues.lock().await;

            let existed = queues.contains_key(&notify_scope_key);
            queues
                .entry(notify_scope_key.clone())
                .or_default()
                .push_back(task.clone());
            if !existed {
                rr.push_back(notify_scope_key.clone());
            }
        }

        {
            let mut active = self.active_tasks.lock().await;
            active.insert(task_id.clone(), task);
        }

        {
            self.proc.write().await.create(
                ProcKind::Tasks,
                &task_id,
                json!({
                    "id": task_id,
                    "head_id": head_id,
                    "goal": goal_text,
                    "scope": scope.to_string(),
                    "status": "queued",
                }),
            );
        }

        tracing::debug!(
            task_id = %task_id,
            head_id = %head_id,
            "task queued"
        );

        self.dispatch_notify.notify_one();
    }

    async fn try_dispatch(&self) -> bool {
        let (hand_id, task) = {
            let mut hands = self.hands.lock().await;
            let mut rr = self.rr_scopes.lock().await;
            let mut queues = self.queues.lock().await;

            let idle_hand = hands
                .iter()
                .find(|h| h.state == HandState::Idle)
                .map(|h| h.hand_id.clone());

            let Some(hand_id) = idle_hand else {
                return false;
            };

            let mut task: Option<Task> = None;
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
                    task = Some(g);
                    break;
                }

                queues.remove(&scope_key);
            }

            let Some(task) = task else {
                return false;
            };

            if let Some(hand) = hands.iter_mut().find(|h| h.hand_id == hand_id) {
                hand.state = HandState::Running {
                    task_id: task.id.clone(),
                    head_id: task.head_id.clone(),
                    started_at: Instant::now(),
                };
            }

            (hand_id, task)
        };

        tracing::info!(
            hand = %hand_id,
            goal = %truncate(&task.goal, 80),
            "task dispatched"
        );

        self.proc.write().await.update(
            ProcKind::Tasks,
            &task.id,
            json!({
                "status": "running",
                "hand_id": hand_id,
            }),
        );

        let assigned_msg = respond::task_assigned(
            "task_service",
            task.scope.clone(),
            task.id.clone(),
            task.head_id.clone(),
            hand_id.clone(),
        )
        .with_origin(Origin::System);

        self.bus.publish(assigned_msg).await;

        true
    }

    async fn handle_result(&self, task_id: String, hand_id: String, ok: bool, summary: String) {
        let task = {
            let mut active = self.active_tasks.lock().await;
            active.remove(&task_id)
        };

        {
            let mut hands = self.hands.lock().await;
            if let Some(hand) = hands.iter_mut().find(|h| h.hand_id == hand_id) {
                hand.state = HandState::Idle;
            }
        }

        self.dispatch_notify.notify_one();

        let status = if ok { "completed" } else { "failed" };

        if let Some(task) = task {
            tracing::info!(
                hand = %hand_id,
                ok = ok,
                goal = %truncate(&task.goal, 80),
                "task {}", status
            );
            let notify_scope_key = task.notify_scope.as_deref().unwrap_or("main").to_string();

            let drained = self
                .decrement_outstanding(&task.head_id, &notify_scope_key)
                .await;
            self.notify_head_finished(&task, ok, &summary).await;
            if drained {
                self.notify_head_drained(&task.head_id, &notify_scope_key, task.reply_to)
                    .await;
            }
        } else {
            tracing::warn!(
                task_id = %task_id,
                hand_id = %hand_id,
                ok = ok,
                "task {} but task not found in active_tasks", status
            );
        }

        self.proc.write().await.update(
            ProcKind::Tasks,
            &task_id,
            json!({
                "status": status,
                "ok": ok,
                "summary": summary,
            }),
        );
    }

    async fn check_timeouts(&self) {
        let now = Instant::now();
        let timeout = Duration::from_secs(self.timeout_secs);

        let timed_out: Vec<(String, String, String, Scope, Option<String>, Option<Uuid>)> = {
            let hands = self.hands.lock().await;
            let active = self.active_tasks.lock().await;
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
                            let (head_id, scope, notify_scope, reply_to) = active
                                .get(task_id)
                                .map(|t| {
                                    (
                                        t.head_id.clone(),
                                        t.scope.clone(),
                                        t.notify_scope.clone(),
                                        t.reply_to,
                                    )
                                })
                                .unwrap_or((
                                    "_unknown".to_string(),
                                    Scope::task(task_id),
                                    None,
                                    None,
                                ));
                            return Some((
                                h.hand_id.clone(),
                                task_id.clone(),
                                head_id,
                                scope,
                                notify_scope,
                                reply_to,
                            ));
                        }
                    }
                    None
                })
                .collect()
        };

        for (hand_id, task_id, head_id, scope, notify_scope, reply_to) in timed_out {
            tracing::warn!(
                task_id = %task_id,
                hand_id = %hand_id,
                timeout_secs = self.timeout_secs,
                "task timed out"
            );

            {
                let mut hands = self.hands.lock().await;
                if let Some(hand) = hands.iter_mut().find(|h| h.hand_id == hand_id) {
                    hand.state = HandState::Idle;
                }
            }

            self.dispatch_notify.notify_one();

            {
                let mut active = self.active_tasks.lock().await;
                if active.remove(&task_id).is_none() {
                    tracing::warn!(
                        task_id = %task_id,
                        hand_id = %hand_id,
                        "task timed out but task not found in active_tasks"
                    );
                }
            }

            let timeout_summary = format!("timed out after {}s", self.timeout_secs);

            // Best-effort cleanup in case the task was still present in a queue.
            {
                let notify_scope_key = notify_scope.as_deref().unwrap_or("main").to_string();
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

            self.bus
                .publish(
                    respond::task_result(
                        "task_service",
                        scope,
                        task_id.clone(),
                        hand_id.clone(),
                        false,
                        format!("FAILED: task {}", timeout_summary),
                    )
                    .with_origin(Origin::System),
                )
                .await;

            let notify_scope_key = notify_scope.as_deref().unwrap_or("main").to_string();
            let drained = self
                .decrement_outstanding(&head_id, &notify_scope_key)
                .await;
            self.notify_head_timeout(&head_id, &task_id, &notify_scope_key, reply_to, &task_short)
                .await;
            if drained {
                self.notify_head_drained(&head_id, &notify_scope_key, reply_to)
                    .await;
            }

            self.proc.write().await.update(
                ProcKind::Tasks,
                &task_id,
                json!({
                    "status": "timeout",
                    "ok": false,
                    "summary": timeout_summary,
                }),
            );
        }
    }

    async fn increment_outstanding(&self, head_id: &str, notify_scope: &str) {
        let mut map = self.outstanding_by_head_notify.lock().await;
        *map.entry(OutstandingKey {
            head_id: head_id.to_string(),
            notify_scope: notify_scope.to_string(),
        })
        .or_insert(0) += 1;
    }

    async fn decrement_outstanding(&self, head_id: &str, notify_scope: &str) -> bool {
        let mut map = self.outstanding_by_head_notify.lock().await;
        let key = OutstandingKey {
            head_id: head_id.to_string(),
            notify_scope: notify_scope.to_string(),
        };
        let Some(v) = map.get_mut(&key) else {
            return false;
        };

        if *v == 0 {
            return false;
        }

        *v -= 1;
        if *v == 0 {
            map.remove(&key);
            return true;
        }

        false
    }

    async fn notify_head_drained(&self, head_id: &str, notify_scope: &str, reply_to: Option<Uuid>) {
        let scope = Scope::head_mail(head_id);
        let payload = json!({ "scope": notify_scope });
        let mut msg = respond::event("task_service", scope, "tasks_drained", payload)
            .with_origin(Origin::System);
        if let Some(reply_to) = reply_to {
            msg = msg.with_reply_to(reply_to);
        }
        self.bus.publish(msg).await;
    }

    async fn notify_head_finished(&self, task: &Task, ok: bool, summary: &str) {
        let notify_scope = task.notify_scope.as_deref().unwrap_or("main");
        let scope = Scope::head_mail(&task.head_id);
        let status = if ok { "completed" } else { "failed" };
        let text = if ok {
            format!(
                "Task {} (task={} scope={}): {}\nResult: {}",
                status,
                task.id,
                notify_scope,
                truncate(&task.goal, 120),
                truncate(summary, 400)
            )
        } else {
            format!(
                "Task {} (task={} scope={}): {}\nError: {}",
                status,
                task.id,
                notify_scope,
                truncate(&task.goal, 120),
                truncate(summary, 400)
            )
        };

        let mut msg = respond::chat("task_service", scope, text).with_origin(Origin::System);
        if let Some(reply_to) = task.reply_to {
            msg = msg.with_reply_to(reply_to);
        }
        self.bus.publish(msg).await;
    }

    async fn notify_head_timeout(
        &self,
        head_id: &str,
        task_id: &str,
        notify_scope: &str,
        reply_to: Option<Uuid>,
        task_short: &str,
    ) {
        let scope = Scope::head_mail(head_id);
        let text = format!(
            "Task timed out ({}s) (task={} scope={}): task {}",
            self.timeout_secs, task_id, notify_scope, task_short
        );
        let mut msg = respond::chat("task_service", scope, text).with_origin(Origin::System);
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

    /// Create a read-only query handle for introspecting task state.
    pub fn query_handle(&self) -> TaskServiceQuery {
        TaskServiceQuery {
            queues: self.queues.clone(),
            hands: self.hands.clone(),
            active_tasks: self.active_tasks.clone(),
        }
    }

    pub async fn cancel_task(&self, task_id: &str) -> bool {
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
            let task = {
                let mut active = self.active_tasks.lock().await;
                active.remove(task_id)
            };
            tracing::debug!(task_id = %task_id, "task cancelled from queue");

            if let Some(task) = task {
                self.bus
                    .publish(
                        respond::task_result(
                            "task_service",
                            task.scope.clone(),
                            task.id.clone(),
                            "unassigned",
                            false,
                            "FAILED: task cancelled".to_string(),
                        )
                        .with_origin(Origin::System),
                    )
                    .await;

                let notify_scope_key = task.notify_scope.as_deref().unwrap_or("main").to_string();
                let drained = self
                    .decrement_outstanding(&task.head_id, &notify_scope_key)
                    .await;
                if drained {
                    self.notify_head_drained(&task.head_id, &notify_scope_key, task.reply_to)
                        .await;
                }
            }

            self.proc.write().await.update(
                ProcKind::Tasks,
                task_id,
                json!({
                    "status": "cancelled",
                    "ok": false,
                    "summary": "task cancelled",
                }),
            );
        }

        if found {
            return true;
        }

        // If the task is currently running, request cancellation from the hand.
        let active_task = {
            let active = self.active_tasks.lock().await;
            active.get(task_id).cloned()
        };

        let Some(_task) = active_task else {
            return false;
        };

        tracing::debug!(task_id = %task_id, "task cancellation requested");
        self.bus
            .publish(
                respond::task_cancel(
                    "task_service",
                    Scope::task(task_id),
                    task_id.to_string(),
                    "cancelled".to_string(),
                )
                .with_origin(Origin::System),
            )
            .await;

        self.proc.write().await.update(
            ProcKind::Tasks,
            task_id,
            json!({
                "status": "cancelling",
            }),
        );

        true
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
