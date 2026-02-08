//! Room:Context - Retrieve conversation history for a scope
//!
//! Enables cross-room context awareness by returning conversation history
//! from any scope. An agent in `room/imessage` can inspect what happened
//! in `room/issue-42` without being a participant in that room.
//!
//! Read-only — no mutation lock required.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::frame_select::{self, FrameSelectArgs};
use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct RoomContext;

impl Default for RoomContext {
    fn default() -> Self {
        Self::new()
    }
}

impl RoomContext {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomContext {
    fn name(&self) -> &'static str {
        "room:context"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::invalid_args("scope is required"))?;

        let limit = data
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(100)
            .clamp(1, 200);

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let frames = k
            .frames()
            .ok_or_else(|| KernelError::internal("frame store not attached"))?;

        let args = FrameSelectArgs {
            scope: Some(scope.to_string()),
            limit: Some(limit),
            order: Some("asc".to_string()),
            ..Default::default()
        };

        let (items, _max_seq) = frame_select::select_conversation(frames.pool(), &args)
            .await
            .map_err(|e| KernelError::internal(format!("conversation query failed: {e}")))?;

        let count = items.len();
        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "scope": scope,
                    "messages": items,
                    "count": count,
                }),
            ))
            .await;
        Ok(())
    }
}
