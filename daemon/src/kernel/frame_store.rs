use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, mpsc};

use crate::kernel::Frame;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredFrame {
    pub seq: u64,
    pub ts_ms: i64,
    pub frame: Frame,
}

#[derive(Debug)]
pub struct FrameStore {
    db_path: PathBuf,
    notify: Notify,
    last_seq: AtomicU64,
    tx: mpsc::Sender<Frame>,
}

impl FrameStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<Self>, rusqlite::Error> {
        let db_path = path.as_ref().to_path_buf();
        let (tx, rx) = mpsc::channel::<Frame>(4096);

        let this = Arc::new(Self {
            db_path,
            notify: Notify::new(),
            last_seq: AtomicU64::new(0),
            tx,
        });

        // Initialize schema synchronously.
        {
            let conn = Connection::open(&this.db_path)?;
            Self::ensure_schema(&conn)?;
            let seq: u64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(seq), 0) FROM frames",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0)
                .max(0) as u64;
            this.last_seq.store(seq, Ordering::Relaxed);
        }

        let writer = this.clone();
        std::thread::spawn(move || {
            writer.writer_loop_blocking(rx);
        });

        Ok(this)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq.load(Ordering::Relaxed)
    }

    pub async fn append(&self, frame: Frame) {
        // Backpressure here is intentional: no lossy logging.
        let _ = self.tx.send(frame).await;
    }

    pub async fn wait_for_seq(&self, after_seq: u64) {
        loop {
            if self.last_seq() > after_seq {
                return;
            }
            self.notify.notified().await;
        }
    }

    pub fn read_since(
        &self,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<StoredFrame>, rusqlite::Error> {
        let conn = Connection::open(&self.db_path)?;
        let mut stmt = conn.prepare(
            "SELECT seq, ts_ms, frame_json FROM frames WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2",
        )?;
        let mut rows = stmt.query(params![after_seq as i64, limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let ts_ms: i64 = row.get(1)?;
            let frame_json: String = row.get(2)?;
            let frame: Frame = serde_json::from_str(&frame_json).unwrap_or_else(|_| {
                Frame::error(
                    uuid::Uuid::new_v4(),
                    serde_json::json!({"code": "E_LOG_PARSE", "message": "failed to parse frame"}),
                )
            });
            out.push(StoredFrame {
                seq: seq.max(0) as u64,
                ts_ms,
                frame,
            });
        }
        Ok(out)
    }

    /// Read the most recent N frames, excluding ticks.
    pub fn read_recent(&self, limit: usize) -> Result<Vec<StoredFrame>, rusqlite::Error> {
        let conn = Connection::open(&self.db_path)?;
        // Subquery to get the last N non-tick frames, then order ascending for proper display
        let mut stmt = conn.prepare(
            "SELECT seq, ts_ms, frame_json FROM (
                SELECT seq, ts_ms, frame_json FROM frames
                WHERE name IS NULL OR name != 'tick'
                ORDER BY seq DESC
                LIMIT ?1
            ) ORDER BY seq ASC",
        )?;
        let mut rows = stmt.query(params![limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let ts_ms: i64 = row.get(1)?;
            let frame_json: String = row.get(2)?;
            let frame: Frame = serde_json::from_str(&frame_json).unwrap_or_else(|_| {
                Frame::error(
                    uuid::Uuid::new_v4(),
                    serde_json::json!({"code": "E_LOG_PARSE", "message": "failed to parse frame"}),
                )
            });
            out.push(StoredFrame {
                seq: seq.max(0) as u64,
                ts_ms,
                frame,
            });
        }
        Ok(out)
    }

    fn writer_loop_blocking(self: Arc<Self>, mut rx: mpsc::Receiver<Frame>) {
        // Single writer connection.
        let conn = match Connection::open(&self.db_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, path = %self.db_path.display(), "failed to open frames db");
                return;
            }
        };
        if let Err(e) = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;") {
            tracing::warn!(error = %e, "failed to configure frames db pragmas");
        }

        loop {
            let frame = rx.blocking_recv();
            let Some(frame) = frame else { return; };
            if let Err(e) = Self::insert_frame(&conn, &frame) {
                tracing::error!(error = %e, "failed to insert kernel frame");
                continue;
            }

            let seq: u64 = conn
                .query_row("SELECT last_insert_rowid()", [], |row| row.get::<_, i64>(0))
                .unwrap_or(0)
                .max(0) as u64;
            self.last_seq.store(seq, Ordering::Relaxed);
            self.notify.notify_waiters();
        }
    }

    fn ensure_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS frames (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_ms INTEGER NOT NULL,
                op TEXT NOT NULL,
                name TEXT,
                actor TEXT,
                frame_id TEXT NOT NULL,
                parent_id TEXT,
                scope TEXT,
                kind TEXT,
                reply_to TEXT,
                frame_json TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_frames_parent ON frames(parent_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_frames_op ON frames(op)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_frames_scope_seq ON frames(scope, seq)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_frames_kind_seq ON frames(kind, seq)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_frames_reply_to ON frames(reply_to)",
            [],
        )?;
        Ok(())
    }

    fn insert_frame(conn: &Connection, frame: &Frame) -> Result<(), rusqlite::Error> {
        let ts_ms = frame.ts;
        let op = format!("{:?}", frame.op);
        let name = frame.name.clone().unwrap_or_default();
        let actor = frame.actor.clone().unwrap_or_default();
        let frame_id = frame.id.to_string();
        let parent_id = frame.parent_id.map(|u| u.to_string()).unwrap_or_default();
        let frame_json = serde_json::to_string(frame).unwrap_or_else(|_| "{}".to_string());

        let (scope, kind, reply_to) = extract_index_fields(frame);

        conn.execute(
            "INSERT INTO frames (ts_ms, op, name, actor, frame_id, parent_id, scope, kind, reply_to, frame_json)
             VALUES (?1, ?2, NULLIF(?3,''), NULLIF(?4,''), ?5, NULLIF(?6,''), NULLIF(?7,''), NULLIF(?8,''), NULLIF(?9,''), ?10)",
            params![
                ts_ms,
                op,
                name,
                actor,
                frame_id,
                parent_id,
                scope.unwrap_or_default(),
                kind.unwrap_or_default(),
                reply_to.unwrap_or_default(),
                frame_json
            ],
        )?;
        Ok(())
    }
}

fn extract_index_fields(frame: &Frame) -> (Option<String>, Option<String>, Option<String>) {
    let data = frame.data.as_ref();

    let mut scope = data
        .and_then(|d| d.get("scope"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if scope.is_none() {
        scope = frame
            .actor
            .as_deref()
            .filter(|s| s.starts_with("session/"))
            .map(|s| s.to_string());
    }

    let kind = if frame.op == crate::kernel::FrameOp::Event {
        data.and_then(|d| d.get("kind"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    };

    let mut reply_to = data
        .and_then(|d| d.get("reply_to"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if reply_to.is_none() && frame.op == crate::kernel::FrameOp::Event {
        reply_to = data
            .and_then(|d| d.get("data"))
            .and_then(|v| v.get("reply_to"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    if reply_to.is_none() {
        reply_to = frame.parent_id.map(|u| u.to_string());
    }

    (scope, kind, reply_to)
}

