use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::bus::{MessageData, MessageOp, Origin, Scope, TaskMsg, respond};

use super::RuntimeBus;

struct TaskMeta {
    head_id: String,
    goal: String,
    completed: bool,
}

pub struct HeadService {
    bus: RuntimeBus,
    head_id: String,
    report_scope: Scope,
    tasks: Arc<Mutex<HashMap<String, TaskMeta>>>,
}

impl HeadService {
    pub fn new(bus: RuntimeBus, head_id: impl Into<String>, report_scope: Scope) -> Self {
        Self {
            bus,
            head_id: head_id.into(),
            report_scope,
            tasks: Arc::new(Mutex::new(HashMap::new())),
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

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if msg.op != MessageOp::Task {
                continue;
            }

            let Scope::Task(_) = msg.scope else {
                continue;
            };

            match msg.data {
                MessageData::Task(TaskMsg::Request { task_id, head_id, goal, input: _ }) => {
                    let mut tasks = self.tasks.lock().unwrap();
                    tasks.entry(task_id).or_insert(TaskMeta {
                        head_id,
                        goal,
                        completed: false,
                    });
                }
                MessageData::Task(TaskMsg::Result { task_id, hand_id, ok, summary }) => {
                    let should_report = {
                        let mut tasks = self.tasks.lock().unwrap();
                        let Some(meta) = tasks.get_mut(&task_id) else {
                            // If we don't have metadata, be conservative and do not report.
                            continue;
                        };
                        if meta.completed {
                            false
                        } else if meta.head_id != self.head_id {
                            false
                        } else {
                            meta.completed = true;
                            true
                        }
                    };

                    if !should_report {
                        continue;
                    }

                    let status = if ok { "OK" } else { "FAILED" };
                    let text = format!(
                        "[task {}] {} (hand {})\n{}",
                        task_id,
                        status,
                        hand_id,
                        summary.trim()
                    );
                    self.bus
                        .publish(
                            respond::chat(self.head_id.clone(), self.report_scope.clone(), text)
                                .with_origin(Origin::Head),
                        )
                        .await;
                }
                _ => {}
            }
        }
    }
}
