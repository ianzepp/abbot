// ExecService handles tool execution requests on the bus.
//
// When a hand decides to use a tool, it publishes an Exec message. This service
// subscribes to all messages, filters for Exec operations, and routes them to
// the appropriate tool via the Dispatcher. Results are published back to the
// same scope. Per-scope working directories are maintained so tools like cd
// affect subsequent tool calls in the same conversation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::bus::{MessageData, MessageOp, Origin, respond};
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

            if msg.origin == Origin::Head && !allowed_head_exec(&tool, &args) {
                let reply = respond::error("tools", scope.clone(), "EPERM", "head may only run read with a direct path")
                    .with_origin(Origin::System)
                    .with_reply_to(exec_id);
                self.bus.publish(reply).await;
                let done = respond::ok_text("tools", scope.clone(), "")
                    .with_origin(Origin::System)
                    .with_reply_to(exec_id);
                self.bus.publish(done).await;
                continue;
            }

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
                respond::error("tools", scope.clone(), "ENOENT", output)
                    .with_origin(Origin::System)
                    .with_reply_to(exec_id)
            } else {
                respond::item_text("tools", scope.clone(), output)
                    .with_origin(Origin::System)
                    .with_reply_to(exec_id)
            };
            self.bus.publish(reply).await;

            let done = respond::ok_text("tools", scope.clone(), "")
                .with_origin(Origin::System)
                .with_reply_to(exec_id);
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

fn allowed_head_exec(tool: &str, args: &str) -> bool {
    if tool != "read" {
        return false;
    }

    let args = args.trim();
    if args.is_empty() {
        return false;
    }

    // Reject obvious globs and traversal.
    if args.contains('*') || args.contains('?') || args.contains('[') || args.contains(']') {
        return false;
    }

    // Parse like: "<path> [offset=N] [limit=N]" (no other flags)
    let mut path = None::<&str>;
    for part in args.split_whitespace() {
        if part.starts_with("offset=") || part.starts_with("limit=") {
            continue;
        }
        if path.is_none() {
            path = Some(part);
        } else {
            return false;
        }
    }

    let Some(path) = path else {
        return false;
    };

    if path.contains("..") {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_exec_policy_allows_read_direct() {
        assert!(allowed_head_exec("read", "Cargo.toml"));
        assert!(allowed_head_exec("read", "/tmp/file.txt"));
        assert!(allowed_head_exec("read", "Cargo.toml offset=1 limit=10"));
    }

    #[test]
    fn head_exec_policy_blocks_unknown_or_unsafe() {
        assert!(!allowed_head_exec("bash", "ls"));
        assert!(!allowed_head_exec("find", "*.rs"));
        assert!(!allowed_head_exec("read", "../secrets.txt"));
        assert!(!allowed_head_exec("read", "**/*.rs"));
        assert!(!allowed_head_exec("read", "path=src Cargo.toml"));
    }
}
