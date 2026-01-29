use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::bus::{MessageData, MessageOp, respond};
use crate::tools::{Dispatcher, ExecutionContext, SharedCwd};

use super::RuntimeBus;

#[derive(Clone)]
pub struct ExecServiceConfig {
    pub max_output_chars: usize,
}

impl Default for ExecServiceConfig {
    fn default() -> Self {
        Self { max_output_chars: 16_000 }
    }
}

pub struct ExecService {
    bus: RuntimeBus,
    dispatcher: Dispatcher,
    config: ExecServiceConfig,
    cwds: Arc<Mutex<HashMap<String, SharedCwd>>>,
}

impl ExecService {
    pub fn new(bus: RuntimeBus, dispatcher: Dispatcher, config: ExecServiceConfig) -> Self {
        Self {
            bus,
            dispatcher,
            config,
            cwds: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(&self) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        tracing::info!("exec service started");

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if msg.op != MessageOp::Exec {
                continue;
            }

            let MessageData::Exec { tool, args } = msg.data.clone() else {
                continue;
            };

            let sender = msg.sender.clone();
            let scope = msg.scope.clone();
            let exec_id = msg.id;

            let ctx = ExecutionContext {
                cwd: self.cwd_for(&sender),
                sender: sender.clone(),
                scope: scope.clone(),
            };

            let output = match self.dispatcher.execute(&tool, &args, &ctx).await {
                Some(out) => out,
                None => format!("unknown command: !{}", tool),
            };

            let output = truncate_chars(&output, self.config.max_output_chars);

            let reply = if output.starts_with("unknown command: !") {
                respond::error("tools", scope.clone(), "ENOENT", output).with_reply_to(exec_id)
            } else {
                respond::item_text("tools", scope.clone(), output).with_reply_to(exec_id)
            };
            self.bus.publish(reply).await;

            let done = respond::ok_text("tools", scope.clone(), "").with_reply_to(exec_id);
            self.bus.publish(done).await;
        }
    }

    fn cwd_for(&self, sender: &str) -> SharedCwd {
        let mut cwds = self.cwds.lock().unwrap();
        cwds.entry(sender.to_string())
            .or_insert_with(|| {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
                Arc::new(Mutex::new(cwd))
            })
            .clone()
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>()
}
