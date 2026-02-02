// NeedService manages the need queue and dispatches work to available heads.
//
// Needs flow from Mind (strategic) or users (reactive) through this service to
// heads. Unlike TaskService which uses FIFO, NeedService uses priority ordering.
// Higher priority needs are dispatched first. Within the same priority, FIFO
// ordering is preserved.

use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::bus::{MessageData, MessageOp, NeedMsg, NeedPriority, Origin, Scope, respond};

use super::RuntimeBus;

const DEFAULT_HEAD_POOL_SIZE: usize = 3;
const DEFAULT_NEED_TIMEOUT_SECS: u64 = 600; // 10 minutes

#[derive(Clone, Debug)]
pub struct Need {
    pub id: String,
    pub source: String,
    pub priority: NeedPriority,
    pub need: String,
    pub context: String,
    pub scope: Scope,
    pub reply_to: Option<Uuid>,
    pub reconvene: bool,
    pub created_at: Instant,
}

impl Eq for Need {}

impl PartialEq for Need {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl PartialOrd for Need {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Need {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.priority.cmp(&other.priority) {
            std::cmp::Ordering::Equal => other.created_at.cmp(&self.created_at),
            other_ord => other_ord,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HeadState {
    Available,
    Processing {
        need_id: String,
        started_at: Instant,
    },
}

#[derive(Clone, Debug)]
pub struct HeadInfo {
    pub head_id: String,
    pub state: HeadState,
}

pub struct NeedService {
    bus: RuntimeBus,
    queue: Arc<Mutex<BinaryHeap<Need>>>,
    heads: Arc<Mutex<Vec<HeadInfo>>>,
    active_needs: Arc<Mutex<HashMap<String, Need>>>,
    dispatch_notify: Arc<Notify>,
    pool_size: usize,
    timeout_secs: u64,
}

impl NeedService {
    pub fn new(bus: RuntimeBus) -> Self {
        Self::with_config(bus, DEFAULT_HEAD_POOL_SIZE, DEFAULT_NEED_TIMEOUT_SECS)
    }

    pub fn with_config(bus: RuntimeBus, pool_size: usize, timeout_secs: u64) -> Self {
        let heads: Vec<HeadInfo> = (0..pool_size)
            .map(|i| HeadInfo {
                head_id: format!("head-{}", i),
                state: HeadState::Available,
            })
            .collect();

        tracing::debug!(
            pool_size = pool_size,
            timeout_secs = timeout_secs,
            "need service configured"
        );

        Self {
            bus,
            queue: Arc::new(Mutex::new(BinaryHeap::new())),
            heads: Arc::new(Mutex::new(heads)),
            active_needs: Arc::new(Mutex::new(HashMap::new())),
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
        tracing::debug!(pool_size = self.pool_size, "need service started");

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            match (&msg.op, &msg.data) {
                (MessageOp::Need, MessageData::Need(need_msg)) => {
                    self.handle_need_msg(msg.scope.clone(), msg.reply_to, need_msg.clone())
                        .await;
                }
                _ => {}
            }
        }
    }

    async fn run_dispatch_loop(&self) {
        loop {
            // Drain dispatch while there is work and capacity.
            while self.try_dispatch().await {}

            // Sleep until new work arrives or a head becomes available.
            self.dispatch_notify.notified().await;
        }
    }

    async fn run_timeout_loop(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            self.check_timeouts().await;
        }
    }

    async fn handle_need_msg(&self, scope: Scope, reply_to: Option<Uuid>, need_msg: NeedMsg) {
        match need_msg {
            NeedMsg::Request {
                need_id,
                source,
                priority,
                need,
                context,
                reconvene,
            } => {
                self.enqueue_need(scope, reply_to, need_id, source, priority, need, context, reconvene)
                    .await;
            }
            NeedMsg::Fulfilled {
                need_id,
                head_id,
                summary,
            } => {
                self.handle_fulfilled(need_id, head_id, summary).await;
            }
            _ => {}
        }
    }

    async fn enqueue_need(
        &self,
        scope: Scope,
        reply_to: Option<Uuid>,
        need_id: String,
        source: String,
        priority: NeedPriority,
        need_text: String,
        context: String,
        reconvene: bool,
    ) {
        let need = Need {
            id: need_id.clone(),
            source: source.clone(),
            priority,
            need: need_text.clone(),
            context,
            scope,
            reconvene,
            reply_to,
            created_at: Instant::now(),
        };

        {
            let mut queue = self.queue.lock().await;
            queue.push(need.clone());
        }

        tracing::debug!(
            need_id = %need_id,
            source = %source,
            priority = ?priority,
            scope = %need.scope,
            "need queued"
        );

        {
            let mut active = self.active_needs.lock().await;
            active.insert(need_id.clone(), need);
        }

        // Wake dispatch loop immediately.
        self.dispatch_notify.notify_one();
    }

    async fn try_dispatch(&self) -> bool {
        let available_head = {
            let heads = self.heads.lock().await;
            heads
                .iter()
                .find(|h| h.state == HeadState::Available)
                .map(|h| h.head_id.clone())
        };

        let Some(head_id) = available_head else {
            return false;
        };

        let next_need = {
            let mut queue = self.queue.lock().await;
            queue.pop()
        };

        let Some(need) = next_need else {
            return false;
        };

        {
            let mut heads = self.heads.lock().await;
            if let Some(head) = heads.iter_mut().find(|h| h.head_id == head_id) {
                head.state = HeadState::Processing {
                    need_id: need.id.clone(),
                    started_at: Instant::now(),
                };
            }
        }

        tracing::info!(
            head = %head_id,
            need = %truncate(&need.need, 80),
            scope = %need.scope,
            "need dispatched"
        );

        let scope = Scope::head_mail(&head_id);
        let mut msg = respond::need_dispatch(
            "need_service",
            scope,
            &need.id,
            &head_id,
            &need.source,
            need.priority,
            &need.need,
            &need.context,
            &need.scope.to_string(),
        )
        .with_origin(Origin::System);
        if let Some(reply_to) = need.reply_to {
            msg = msg.with_reply_to(reply_to);
        }
        self.bus.publish(msg).await;

        true
    }

    async fn handle_fulfilled(&self, need_id: String, head_id: String, summary: String) {
        let need = {
            let mut active = self.active_needs.lock().await;
            active.remove(&need_id)
        };

        {
            let mut heads = self.heads.lock().await;
            if let Some(head) = heads.iter_mut().find(|h| h.head_id == head_id) {
                head.state = HeadState::Available;
            }
        }

        // Head became available; wake dispatch loop.
        self.dispatch_notify.notify_one();

        if let Some(need) = need {
            tracing::info!(
                head = %head_id,
                need = %truncate(&need.need, 80),
                scope = %need.scope,
                reconvene = need.reconvene,
                "need fulfilled"
            );

            // Notify mind that need was fulfilled
            let scope = Scope::from("@mind");
            let text = format!(
                "Need fulfilled (need_id={} head={}): {}\nSummary: {}",
                need_id,
                head_id,
                truncate(&need.need, 120),
                truncate(&summary, 400)
            );
            let msg = respond::chat("need_service", scope.clone(), text).with_origin(Origin::System);
            self.bus.publish(msg).await;

            // Trigger reconvene if requested
            if need.reconvene {
                tracing::info!(need_id = %need_id, "triggering reconvene");
                let event = respond::event(
                    "need_service",
                    scope,
                    "convene_conclave",
                    serde_json::json!({
                        "reason": format!("Need fulfilled: {}", truncate(&need.need, 80)),
                        "need_id": need_id,
                        "summary": truncate(&summary, 200)
                    }),
                )
                .with_origin(Origin::System);
                self.bus.publish(event).await;
            }
        } else {
            tracing::warn!(
                need_id = %need_id,
                head_id = %head_id,
                "need fulfilled but not found in active_needs"
            );
        }
    }

    async fn check_timeouts(&self) {
        let now = Instant::now();
        let timeout = Duration::from_secs(self.timeout_secs);

        let timed_out: Vec<(String, String)> = {
            let heads = self.heads.lock().await;
            heads
                .iter()
                .filter_map(|h| {
                    if let HeadState::Processing { need_id, started_at } = &h.state {
                        if now.duration_since(*started_at) > timeout {
                            return Some((h.head_id.clone(), need_id.clone()));
                        }
                    }
                    None
                })
                .collect()
        };

        for (head_id, need_id) in timed_out {
            tracing::warn!(
                need_id = %need_id,
                head_id = %head_id,
                timeout_secs = self.timeout_secs,
                "need timed out"
            );

            {
                let mut heads = self.heads.lock().await;
                if let Some(head) = heads.iter_mut().find(|h| h.head_id == head_id) {
                    head.state = HeadState::Available;
                }
            }

            // Head became available; wake dispatch loop.
            self.dispatch_notify.notify_one();

            let need = {
                let mut active = self.active_needs.lock().await;
                active.remove(&need_id)
            };

            let reason = format!("timed out after {}s", self.timeout_secs);
            if let Some(need) = need {
                let mut msg = respond::need_expired("need_service", need.scope.clone(), &need_id, &reason)
                    .with_origin(Origin::System);
                if let Some(reply_to) = need.reply_to {
                    msg = msg.with_reply_to(reply_to);
                }
                self.bus.publish(msg).await;

                let text = format!(
                    "Need expired (need_id={} head={}): {}\nReason: {}",
                    need_id,
                    head_id,
                    truncate(&need.need, 120),
                    reason
                );
                let msg = respond::chat("need_service", Scope::from("@mind"), text)
                    .with_origin(Origin::System);
                self.bus.publish(msg).await;
            } else {
                self.bus
                    .publish(
                        respond::need_expired(
                            "need_service",
                            Scope::from("@need_service"),
                            &need_id,
                            &reason,
                        )
                        .with_origin(Origin::System),
                    )
                    .await;
            }
        }
    }

    pub async fn queue_depth(&self) -> usize {
        self.queue.lock().await.len()
    }

    pub async fn queue_by_priority(&self) -> HashMap<NeedPriority, usize> {
        let queue = self.queue.lock().await;
        let mut counts = HashMap::new();
        for need in queue.iter() {
            *counts.entry(need.priority).or_insert(0) += 1;
        }
        counts
    }

    pub async fn head_status(&self) -> Vec<HeadInfo> {
        self.heads.lock().await.clone()
    }

    pub async fn cancel_need(&self, need_id: &str) -> bool {
        let mut found_in_queue = false;
        let mut found_in_active = false;
        let mut removed_need: Option<Need> = None;

        {
            let mut queue = self.queue.lock().await;
            let old_queue: Vec<Need> = std::mem::take(&mut *queue).into_vec();
            for n in old_queue {
                if n.id == need_id {
                    removed_need = Some(n);
                    found_in_queue = true;
                } else {
                    queue.push(n);
                }
            }
        }

        {
            let mut active = self.active_needs.lock().await;
            if let Some(n) = active.remove(need_id) {
                removed_need = Some(n);
                found_in_active = true;
            }
        }

        if found_in_queue || found_in_active {
            tracing::debug!(
                need_id = %need_id,
                from_queue = found_in_queue,
                from_active = found_in_active,
                scope = %removed_need.as_ref().map(|n| n.scope.to_string()).unwrap_or_else(|| "(unknown)".to_string()),
                "need cancelled"
            );

            let reason = "cancelled";
            if let Some(need) = removed_need {
                let mut msg = respond::need_expired("need_service", need.scope, need_id, reason)
                    .with_origin(Origin::System);
                if let Some(reply_to) = need.reply_to {
                    msg = msg.with_reply_to(reply_to);
                }
                self.bus.publish(msg).await;
            } else {
                self.bus
                    .publish(
                        respond::need_expired("need_service", Scope::from("@need_service"), need_id, reason)
                            .with_origin(Origin::System),
                    )
                    .await;
            }

            // Queue and/or active set changed; wake dispatch loop.
            self.dispatch_notify.notify_one();
        }

        found_in_queue || found_in_active
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
