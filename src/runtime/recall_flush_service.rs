use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use crate::bus::{MessageData, MessageOp, Origin, TaskMsg, NeedMsg, WantMsg};
use crate::history::Store;
use crate::recall::{ensure_schema as ensure_recall_schema, Indexer, Ollama};
use crate::runtime::{
    atomic_write_file_0600,
    workspace_dir_from_root,
    workspace_name_from_root,
    workspace_transcripts_dir,
};

use super::RuntimeBus;

pub struct RecallFlushService {
    bus: RuntimeBus,
    store: Arc<Store>,
    workspace_root: PathBuf,
    running: AtomicBool,
    pending: AtomicBool,
    last_idle_ms: AtomicI64,
}

impl RecallFlushService {
    pub fn new(bus: RuntimeBus, store: Arc<Store>, workspace_root: PathBuf) -> Self {
        Self {
            bus,
            store,
            workspace_root,
            running: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            last_idle_ms: AtomicI64::new(0),
        }
    }

    pub fn start(self: Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            this.run().await;
        });
    }

    async fn run(self: Arc<Self>) {
        let mut rx = self.bus.hub().read().await.subscribe_all();
        tracing::debug!("recall flush service started");

        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if msg.op == MessageOp::Event && msg.origin == Origin::System {
                if let MessageData::Event { kind, .. } = &msg.data {
                    if kind == "slow_idle" {
                        let idle_ts_ms = msg
                            .timestamp
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64;
                        self.trigger_flush(idle_ts_ms);
                    }
                }
            }
        }
    }

    fn trigger_flush(self: &Arc<Self>, idle_ts_ms: i64) {
        self.last_idle_ms.store(idle_ts_ms, Ordering::SeqCst);

        if self.running.load(Ordering::SeqCst) {
            self.pending.store(true, Ordering::SeqCst);
            return;
        }

        if self.running.swap(true, Ordering::SeqCst) {
            self.pending.store(true, Ordering::SeqCst);
            return;
        }

        let this = self.clone();
        tokio::spawn(async move {
            loop {
                let idle = this.last_idle_ms.load(Ordering::SeqCst);
                if let Err(e) = this.flush_once(idle).await {
                    tracing::warn!(error = %e, "recall flush failed");
                }

                if this.pending.swap(false, Ordering::SeqCst) {
                    continue;
                }
                break;
            }

            this.running.store(false, Ordering::SeqCst);
        });
    }

    async fn flush_once(&self, idle_ts_ms: i64) -> Result<(), String> {
        let workspace_dir = workspace_dir_from_root(&self.workspace_root);
        let recall_db = workspace_dir.join("recall.db");

        let conn = rusqlite::Connection::open(&recall_db)
            .map_err(|e| format!("failed to open recall db: {e}"))?;

        ensure_recall_schema(&conn)
            .map_err(|e| format!("failed to ensure recall schema: {e}"))?;

        let last_flushed_ms = get_recall_state_i64(&conn, "last_flushed_ts_ms")
            .unwrap_or(0);

        if idle_ts_ms <= last_flushed_ms {
            return Ok(());
        }

        // Pull new bus messages from store.sqlite.
        let mut start_ms = last_flushed_ms;
        let mut collected = Vec::new();
        let limit = 5_000usize;
        loop {
            let batch = self
                .store
                .messages_between_ms(start_ms, idle_ts_ms, limit)
                .map_err(|e| format!("failed to query messages: {e}"))?;
            if batch.is_empty() {
                break;
            }

            start_ms = batch
                .last()
                .and_then(|m| {
                    m.timestamp
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_millis() as i64)
                })
                .unwrap_or(start_ms);

            collected.extend(batch);

            if start_ms >= idle_ts_ms {
                break;
            }
        }

        if collected.is_empty() {
            set_recall_state_i64(&conn, "last_flushed_ts_ms", idle_ts_ms)
                .map_err(|e| format!("failed to update recall cursor: {e}"))?;
            return Ok(());
        }

        // Materialize transcript segment.
        let transcripts_dir = workspace_transcripts_dir(&self.workspace_root);
        let file_path = transcripts_dir.join(format!("idle-{}.txt", idle_ts_ms));
        let started = chrono::Utc::now().to_rfc3339();

        let workspace_name = workspace_name_from_root(&self.workspace_root);
        let mut out = String::new();
        out.push_str(&format!("📋 Session: workspace:{}\n", workspace_name));
        out.push_str(&format!("📋 Project: {}\n", self.workspace_root.display()));
        out.push_str(&format!("📋 Started: {}\n", started));
        out.push_str(&format!(
            "📋 Range: {}..={}\n\n",
            last_flushed_ms + 1,
            idle_ts_ms
        ));

        for msg in &collected {
            if let Some(line) = render_recall_line(msg) {
                out.push_str(&line);
                out.push('\n');
            }
        }

        atomic_write_file_0600(&file_path, &out)
            .map_err(|e| format!("failed to write transcript: {e}"))?;

        // Index the transcript segment.
        let indexer = Indexer::new(conn, Ollama::local());
        indexer
            .index_file(&file_path)
            .await
            .map_err(|e| format!("failed to index transcript: {e}"))?;

        // Advance cursor only after successful indexing.
        let conn = rusqlite::Connection::open(&recall_db)
            .map_err(|e| format!("failed to reopen recall db: {e}"))?;
        ensure_recall_schema(&conn)
            .map_err(|e| format!("failed to ensure recall schema: {e}"))?;
        set_recall_state_i64(&conn, "last_flushed_ts_ms", idle_ts_ms)
            .map_err(|e| format!("failed to update recall cursor: {e}"))?;

        Ok(())
    }
}

fn set_recall_state_i64(conn: &rusqlite::Connection, key: &str, value: i64) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO recall_state (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value.to_string()],
    )?;
    Ok(())
}

fn get_recall_state_i64(conn: &rusqlite::Connection, key: &str) -> Option<i64> {
    let mut stmt = conn
        .prepare("SELECT value FROM recall_state WHERE key = ?1")
        .ok()?;
    let v: String = stmt.query_row(rusqlite::params![key], |row| row.get(0)).ok()?;
    v.parse::<i64>().ok()
}

fn render_recall_line(msg: &crate::bus::Message) -> Option<String> {
    let scope = msg.scope.to_string();

    match (&msg.op, &msg.data) {
        (MessageOp::Chat, MessageData::Text(t)) => {
            let prefix = match msg.origin {
                Origin::Human => "👤",
                Origin::Head => "🤖",
                Origin::Hand => "🤖",
                Origin::System => "📋",
            };
            Some(format!("{} [{}] {}: {}", prefix, scope, msg.sender, t))
        }

        (MessageOp::Task, MessageData::Task(task_msg)) => match task_msg {
            TaskMsg::Request { task_id, head_id, goal, .. } => Some(format!(
                "📋 [task {}] requested by {}: {}",
                short(task_id),
                head_id,
                goal
            )),
            TaskMsg::ToolCall { task_id, tool, args, .. } => {
                let preview: String = args.to_string().chars().take(500).collect();
                Some(format!("✅ [task {}] tool_call {} {}", short(task_id), tool, preview))
            }
            TaskMsg::ToolDone { task_id, tool, ok, duration_ms, error_code, .. } => {
                if *ok {
                    Some(format!("✅ [task {}] tool_done {} ok ({}ms)", short(task_id), tool, duration_ms))
                } else if let Some(code) = error_code {
                    Some(format!("❌ [task {}] tool_done {} error={} ({}ms)", short(task_id), tool, code, duration_ms))
                } else {
                    Some(format!("❌ [task {}] tool_done {} failed ({}ms)", short(task_id), tool, duration_ms))
                }
            }
            TaskMsg::Result { task_id, ok, summary, .. } => {
                let prefix = if *ok { "✅" } else { "❌" };
                Some(format!("{} [task {}] result: {}", prefix, short(task_id), summary))
            }
            TaskMsg::Progress { .. } => None,
            TaskMsg::Assigned { .. } => None,
            TaskMsg::Echo { .. } => None,
            TaskMsg::Cancel { .. } => None,
        },

        (MessageOp::Need, MessageData::Need(need_msg)) => match need_msg {
            NeedMsg::Request { need_id, priority, need, .. } => Some(format!(
                "📋 [need {}] request {:?}: {}",
                short(need_id),
                priority,
                need
            )),
            NeedMsg::Fulfilled { need_id, head_id, summary, .. } => Some(format!(
                "📋 [need {}] fulfilled by {}: {}",
                short(need_id),
                head_id,
                summary
            )),
            NeedMsg::Expired { need_id, reason } => Some(format!(
                "📋 [need {}] expired: {}",
                short(need_id),
                reason
            )),
            NeedMsg::Acknowledged { .. } => None,
            NeedMsg::Dispatch { .. } => None,
        },

        (MessageOp::Want, MessageData::Want(want_msg)) => match want_msg {
            WantMsg::Added { want_id, priority, want, .. } => Some(format!(
                "📋 [want {}] added [{}]: {}",
                short(want_id),
                priority,
                want
            )),
            WantMsg::Removed { want_id, reason } => Some(format!(
                "📋 [want {}] removed: {}",
                short(want_id),
                reason
            )),
            WantMsg::Promoted { want_id, to_priority, need_id } => {
                if let Some(need_id) = need_id {
                    Some(format!(
                        "📋 [want {}] promoted -> {} (need {})",
                        short(want_id),
                        to_priority,
                        short(need_id)
                    ))
                } else {
                    Some(format!(
                        "📋 [want {}] promoted -> {}",
                        short(want_id),
                        to_priority
                    ))
                }
            }
        },

        _ => None,
    }
}

fn short(s: &str) -> String {
    s.chars().take(8).collect()
}
