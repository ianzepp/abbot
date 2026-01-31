use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rand::RngCore;

use crate::bus::{MessageData, MessageOp, Origin, TaskMsg, respond};

use super::RuntimeBus;

pub struct HandAllocator {
    bus: RuntimeBus,
    assigned: Arc<Mutex<HashSet<String>>>,
}

impl HandAllocator {
    pub fn new(bus: RuntimeBus) -> Self {
        Self {
            bus,
            assigned: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(&self) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        tracing::info!("hand allocator started");

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if msg.op != MessageOp::Task {
                continue;
            }
            if !msg.scope.is_task() {
                continue;
            }

            let MessageData::Task(TaskMsg::Request { task_id, head_id, .. }) = msg.data.clone() else {
                continue;
            };

            {
                let mut assigned = self.assigned.lock().expect("hand allocator lock poisoned");
                if !assigned.insert(task_id.clone()) {
                    continue;
                }
            }

            let hand_id = format!("hand-{}", random_hex8());
            let assigned = respond::task_assigned(
                "_allocator",
                msg.scope.clone(),
                task_id,
                head_id,
                hand_id,
            )
            .with_origin(Origin::System);
            self.bus.publish(assigned).await;
        }
    }
}

fn random_hex8() -> String {
    let mut rng = rand::rng();
    let n = rng.next_u32();
    format!("{:08x}", n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::RwLock;
    use crate::bus::{Hub, Scope};
    use crate::history::Store;
    use crate::runtime::RuntimeBus;

    #[tokio::test]
    async fn assigns_hand_on_task_request() {
        let store = Arc::new(Store::open(":memory:").unwrap());
        let hub = Arc::new(RwLock::new(Hub::new()));
        let bus = RuntimeBus::new(hub.clone(), store);

        let allocator = Arc::new(HandAllocator::new(bus.clone()));
        allocator.start();
        tokio::time::sleep(Duration::from_millis(10)).await;

        let scope = Scope::task("test-1");
        bus.create_scope(scope.clone()).await;
        let mut rx = bus.hub().read().await.subscribe(&scope).unwrap();

        let req = respond::task_request(
            "head",
            scope.clone(),
            "test-1",
            "head",
            "do thing",
            "input",
        )
        .with_origin(crate::bus::Origin::Head);
        bus.publish(req).await;

        let assigned = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = rx.recv().await.unwrap();
                if msg.op != MessageOp::Task {
                    continue;
                }
                if let MessageData::Task(TaskMsg::Assigned { hand_id, .. }) = msg.data {
                    return hand_id;
                }
            }
        })
        .await
        .expect("timed out waiting for assignment");

        assert!(assigned.starts_with("hand-"));
    }
}
