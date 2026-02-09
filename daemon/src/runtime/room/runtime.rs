//! Room runtime abstraction for testable execution.

use std::path::PathBuf;

use async_trait::async_trait;
use uuid::Uuid;

use crate::kernel::{Frame, KernelReceiver};
use crate::runtime::Kernel;

/// Frame info for user messages in a room.
#[derive(Debug, Clone)]
pub struct UserMessageFrame {
    pub seq: i64,
    pub frame_json: String,
}

#[async_trait]
pub trait RoomRuntime: Send + Sync {
    fn workspace(&self) -> PathBuf;
    fn current_frame_seq(&self) -> i64;

    async fn emit_frame(&self, room: &str, thread_id: Uuid, frame: Frame);

    async fn dispatch(&self, req: Frame, workspace: PathBuf) -> Result<KernelReceiver, String>;

    async fn fetch_user_message_frames(
        &self,
        room: &str,
        last_seq: i64,
    ) -> Result<Vec<UserMessageFrame>, String>;
}

pub struct KernelRoomRuntime;

impl Default for KernelRoomRuntime {
    fn default() -> Self {
        Self
    }
}

impl KernelRoomRuntime {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl RoomRuntime for KernelRoomRuntime {
    fn workspace(&self) -> PathBuf {
        Kernel::get()
            .map(|k| k.workspace().to_path_buf())
            .unwrap_or_default()
    }

    fn current_frame_seq(&self) -> i64 {
        Kernel::get()
            .and_then(|k| k.frames())
            .map(|s| s.last_seq() as i64)
            .unwrap_or(0)
    }

    async fn emit_frame(&self, room: &str, thread_id: Uuid, frame: Frame) {
        let Some(k) = Kernel::get() else { return };
        k.sigcalls().send(room, thread_id, frame).await;
    }

    async fn dispatch(&self, req: Frame, workspace: PathBuf) -> Result<KernelReceiver, String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        Ok(dispatcher.dispatch(req, workspace, tokio_util::sync::CancellationToken::new()))
    }

    async fn fetch_user_message_frames(
        &self,
        room: &str,
        last_seq: i64,
    ) -> Result<Vec<UserMessageFrame>, String> {
        let Some(k) = Kernel::get() else {
            return Ok(Vec::new());
        };
        let Some(store) = k.frames() else {
            return Ok(Vec::new());
        };
        let pool = store.pool();

        let rows = sqlx::query(
            "SELECT seq, frame_json FROM frames \
             WHERE seq > ? AND room = ? AND kind = 'chat:user' \
             ORDER BY seq ASC LIMIT 50",
        )
        .bind(last_seq)
        .bind(room)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let seq: i64 = sqlx::Row::get(&row, 0);
            let frame_json: String = sqlx::Row::get(&row, 1);
            out.push(UserMessageFrame { seq, frame_json });
        }
        Ok(out)
    }
}
