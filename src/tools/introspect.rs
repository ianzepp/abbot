// IntrospectTool queries the persisted Store for system state.
//
// Modes:
//   messages [scope=X] [limit=N]  - recent messages
//   wants [limit=N]               - wants pool
//   logs [task=X] [limit=N]       - hand execution logs
//   stats                         - aggregate statistics
//   needs [scope=X] [limit=N]     - need lifecycle messages
//   goals [scope=X] [limit=N]     - goal/task lifecycle messages

use std::sync::Arc;
use std::time::SystemTime;

use crate::bus::MessageOp;
use crate::history::Store;

use super::{ExecutionContext, Tool};

pub struct IntrospectTool {
    store: Arc<Store>,
}

impl IntrospectTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    fn parse_args(args: &str) -> (Option<&str>, Vec<(&str, &str)>) {
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.is_empty() {
            return (None, vec![]);
        }

        let mode = Some(parts[0]);
        let mut params = vec![];

        for part in &parts[1..] {
            if let Some((k, v)) = part.split_once('=') {
                params.push((k, v));
            }
        }

        (mode, params)
    }

    fn get_param<'a>(params: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
        params.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    fn get_limit(params: &[(&str, &str)], default: usize) -> usize {
        Self::get_param(params, "limit")
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    fn format_time(ts: SystemTime) -> String {
        ts.duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| {
                let secs = d.as_secs();
                let hours = (secs / 3600) % 24;
                let mins = (secs / 60) % 60;
                let s = secs % 60;
                format!("{:02}:{:02}:{:02}", hours, mins, s)
            })
            .unwrap_or_else(|_| "??:??:??".to_string())
    }

    async fn introspect_messages(&self, params: &[(&str, &str)]) -> String {
        let limit = Self::get_limit(params, 20);
        let scope = Self::get_param(params, "scope");

        let messages = if let Some(scope) = scope {
            self.store.recent(scope, limit)
        } else {
            self.store.recent_any(limit)
        };

        match messages {
            Ok(msgs) if msgs.is_empty() => "No messages found.".to_string(),
            Ok(msgs) => {
                let mut lines = vec![format!("Recent messages ({}):", msgs.len())];
                for msg in msgs {
                    let ts = Self::format_time(msg.timestamp);
                    let scope = format!("{}", msg.scope);
                    let preview: String = format!("{:?}", msg.data).chars().take(80).collect();
                    lines.push(format!("[{}] {} {} | {}", ts, msg.sender, scope, preview));
                }
                lines.join("\n")
            }
            Err(e) => format!("Error querying messages: {}", e),
        }
    }

    async fn introspect_wants(&self, params: &[(&str, &str)]) -> String {
        let limit = Self::get_limit(params, 20);

        match self.store.list_wants(limit) {
            Ok(wants) if wants.is_empty() => "No wants in pool.".to_string(),
            Ok(wants) => {
                let mut lines = vec![format!("Wants pool ({}):", wants.len())];
                for want in wants {
                    lines.push(format!(
                        "- [{}] {} (priority={}, source={})",
                        &want.id[..8],
                        want.want,
                        want.priority,
                        want.source
                    ));
                }
                lines.join("\n")
            }
            Err(e) => format!("Error querying wants: {}", e),
        }
    }

    async fn introspect_logs(&self, params: &[(&str, &str)]) -> String {
        let task_id = Self::get_param(params, "task");

        if let Some(task_id) = task_id {
            match self.store.get_hand_execs(task_id) {
                Ok(execs) if execs.is_empty() => format!("No logs for task {}.", task_id),
                Ok(execs) => {
                    let mut lines = vec![format!("Logs for task {} ({} steps):", task_id, execs.len())];
                    for exec in execs {
                        let status = if exec.success { "ok" } else { "fail" };
                        let output_preview: String = exec.output.chars().take(60).collect();
                        lines.push(format!(
                            "  [{}] {} {}: {}",
                            exec.step, exec.tool, status, output_preview
                        ));
                    }
                    lines.join("\n")
                }
                Err(e) => format!("Error querying logs: {}", e),
            }
        } else {
            "Usage: !introspect logs task=<task_id>".to_string()
        }
    }

    async fn introspect_stats(&self) -> String {
        let mut lines = vec!["System stats:".to_string()];

        // Wants count
        match self.store.count_wants() {
            Ok(count) => lines.push(format!("  Wants pool: {}", count)),
            Err(e) => lines.push(format!("  Wants pool: error ({})", e)),
        }

        // Recent message counts by op
        if let Ok(msgs) = self.store.recent_any(100) {
            let mut chat_count = 0;
            let mut task_count = 0;
            let mut need_count = 0;
            let mut error_count = 0;

            for msg in &msgs {
                match msg.op {
                    MessageOp::Chat => chat_count += 1,
                    MessageOp::Task => task_count += 1,
                    MessageOp::Need => need_count += 1,
                    MessageOp::Error => error_count += 1,
                    _ => {}
                }
            }

            lines.push(format!("  Recent (last 100): {} chat, {} task, {} need, {} error",
                chat_count, task_count, need_count, error_count));
        }

        // Channels
        if let Ok(channels) = self.store.list_channels() {
            let channel_list: Vec<String> = channels
                .iter()
                .take(5)
                .map(|(name, count)| format!("{}({})", name, count))
                .collect();
            lines.push(format!("  Active channels: {}", channel_list.join(", ")));
        }

        lines.join("\n")
    }

    async fn introspect_needs(&self, params: &[(&str, &str)]) -> String {
        let limit = Self::get_limit(params, 20);
        let scope = Self::get_param(params, "scope").unwrap_or("#general");

        match self.store.recent_by_op(scope, "Need", limit) {
            Ok(msgs) if msgs.is_empty() => "No need messages found.".to_string(),
            Ok(msgs) => {
                let mut lines = vec![format!("Recent need messages ({}):", msgs.len())];
                for msg in msgs {
                    let ts = Self::format_time(msg.timestamp);
                    let preview: String = format!("{:?}", msg.data).chars().take(80).collect();
                    lines.push(format!("[{}] {} | {}", ts, msg.sender, preview));
                }
                lines.join("\n")
            }
            Err(e) => format!("Error querying needs: {}", e),
        }
    }

    async fn introspect_goals(&self, params: &[(&str, &str)]) -> String {
        let limit = Self::get_limit(params, 20);
        let scope = Self::get_param(params, "scope").unwrap_or("#general");

        match self.store.recent_by_op(scope, "Task", limit) {
            Ok(msgs) if msgs.is_empty() => "No goal/task messages found.".to_string(),
            Ok(msgs) => {
                let mut lines = vec![format!("Recent goal/task messages ({}):", msgs.len())];
                for msg in msgs {
                    let ts = Self::format_time(msg.timestamp);
                    let preview: String = format!("{:?}", msg.data).chars().take(80).collect();
                    lines.push(format!("[{}] {} | {}", ts, msg.sender, preview));
                }
                lines.join("\n")
            }
            Err(e) => format!("Error querying goals: {}", e),
        }
    }
}

#[async_trait::async_trait]
impl Tool for IntrospectTool {
    fn name(&self) -> &str {
        "introspect"
    }

    fn description(&self) -> &str {
        "Query system state: messages, wants, logs, stats, needs, goals"
    }

    async fn execute(&self, args: &str, _ctx: &ExecutionContext) -> String {
        let (mode, params) = Self::parse_args(args);

        match mode {
            Some("messages") => self.introspect_messages(&params).await,
            Some("wants") => self.introspect_wants(&params).await,
            Some("logs") => self.introspect_logs(&params).await,
            Some("stats") => self.introspect_stats().await,
            Some("needs") => self.introspect_needs(&params).await,
            Some("goals") => self.introspect_goals(&params).await,
            Some(unknown) => format!("Unknown mode: {}. Use: messages, wants, logs, stats, needs, goals", unknown),
            None => {
                "Usage: !introspect <mode> [params]\n\
                 Modes:\n\
                 - messages [scope=X] [limit=N]\n\
                 - wants [limit=N]\n\
                 - logs task=X\n\
                 - stats\n\
                 - needs [scope=X] [limit=N]\n\
                 - goals [scope=X] [limit=N]".to_string()
            }
        }
    }
}
