use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqliteSynchronous};
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
    pool: SqlitePool,
    db_path: PathBuf,
    notify: Notify,
    last_seq: AtomicU64,
    tx: mpsc::Sender<Frame>,
}

impl FrameStore {
    pub async fn open(path: impl AsRef<Path>) -> Result<Arc<Self>, sqlx::Error> {
        let db_path = path.as_ref().to_path_buf();
        let is_memory = db_path.to_string_lossy() == ":memory:";

        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 4 })
            .connect_with(opts)
            .await?;

        // Create schema
        let schema = "
            CREATE TABLE IF NOT EXISTS frames (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_ms INTEGER NOT NULL,
                op TEXT NOT NULL,
                name TEXT,
                actor TEXT,
                frame_id TEXT NOT NULL,
                parent_id TEXT,
                room TEXT,
                kind TEXT,
                reply_to TEXT,
                frame_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_frames_parent ON frames(parent_id);
            CREATE INDEX IF NOT EXISTS idx_frames_op ON frames(op);
            CREATE INDEX IF NOT EXISTS idx_frames_room_seq ON frames(room, seq);
            CREATE INDEX IF NOT EXISTS idx_frames_kind_seq ON frames(kind, seq);
            CREATE INDEX IF NOT EXISTS idx_frames_reply_to ON frames(reply_to);
        ";
        sqlx::raw_sql(schema).execute(&pool).await?;

        // Read initial max seq
        let seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM frames")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);

        let (tx, rx) = mpsc::channel::<Frame>(4096);

        let this = Arc::new(Self {
            pool,
            db_path,
            notify: Notify::new(),
            last_seq: AtomicU64::new(seq.max(0) as u64),
            tx,
        });

        let writer = this.clone();
        tokio::spawn(async move {
            writer.writer_loop(rx).await;
        });

        Ok(this)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
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

    pub async fn read_since(
        &self,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<StoredFrame>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT seq, ts_ms, frame_json FROM frames WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2",
        )
        .bind(after_seq as i64)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::new();
        for row in rows {
            let seq: i64 = row.get(0);
            let ts_ms: i64 = row.get(1);
            let frame_json: String = row.get(2);
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
    pub async fn read_recent(&self, limit: usize) -> Result<Vec<StoredFrame>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT seq, ts_ms, frame_json FROM (
                SELECT seq, ts_ms, frame_json FROM frames
                WHERE name IS NULL OR name != 'tick'
                ORDER BY seq DESC
                LIMIT ?1
            ) ORDER BY seq ASC",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::new();
        for row in rows {
            let seq: i64 = row.get(0);
            let ts_ms: i64 = row.get(1);
            let frame_json: String = row.get(2);
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

    async fn writer_loop(self: Arc<Self>, mut rx: mpsc::Receiver<Frame>) {
        loop {
            let frame = rx.recv().await;
            let Some(frame) = frame else { return };
            if let Err(e) = self.insert_frame(&frame).await {
                tracing::error!(error = %e, "failed to insert kernel frame");
                continue;
            }
            self.notify.notify_waiters();
        }
    }

    async fn insert_frame(&self, frame: &Frame) -> Result<(), sqlx::Error> {
        let ts_ms = frame.ts;
        let op = format!("{:?}", frame.op);
        let name = frame.name.clone().unwrap_or_default();
        let actor = frame.actor.clone().unwrap_or_default();
        let frame_id = frame.id.to_string();
        let parent_id = frame.parent_id.map(|u| u.to_string()).unwrap_or_default();
        let frame_json = serde_json::to_string(frame).unwrap_or_else(|_| "{}".to_string());

        let (room, kind, reply_to) = extract_index_fields(frame);

        let result = sqlx::query(
            "INSERT INTO frames (ts_ms, op, name, actor, frame_id, parent_id, room, kind, reply_to, frame_json)
             VALUES (?1, ?2, NULLIF(?3,''), NULLIF(?4,''), ?5, NULLIF(?6,''), NULLIF(?7,''), NULLIF(?8,''), NULLIF(?9,''), ?10)",
        )
        .bind(ts_ms)
        .bind(&op)
        .bind(&name)
        .bind(&actor)
        .bind(&frame_id)
        .bind(&parent_id)
        .bind(room.as_deref().unwrap_or(""))
        .bind(kind.as_deref().unwrap_or(""))
        .bind(reply_to.as_deref().unwrap_or(""))
        .bind(&frame_json)
        .execute(&self.pool)
        .await?;

        let seq = result.last_insert_rowid().max(0) as u64;
        self.last_seq.store(seq, Ordering::Relaxed);
        Ok(())
    }
}

fn extract_index_fields(frame: &Frame) -> (Option<String>, Option<String>, Option<String>) {
    let data = frame.data.as_ref();

    let room = data
        .and_then(|d| d.get("room"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let kind = data
        .and_then(|d| d.get("kind"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

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

    (room, kind, reply_to)
}
