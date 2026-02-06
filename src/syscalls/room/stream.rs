use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::kernel::{Frame, FrameOp, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct RoomStream;

impl RoomStream {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomStream {
    fn name(&self) -> &'static str {
        "room:stream"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let room_id = data
            .get("room_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| KernelError::invalid_args("room_id is required"))?;

        if k.rooms().get(room_id).await.is_none() {
            return Err(KernelError::not_found("room not found"));
        }

        let rx = k.rooms().open_stream(room_id).await;
        let mut stream = ReceiverStream::new(rx);

        while let Some(mut frame) = stream.next().await {
            frame.parent_id = Some(ctx.call_id);
            let is_terminal = matches!(frame.op, FrameOp::Ok | FrameOp::Error | FrameOp::Done);
            let _ = tx.send(frame).await;
            if is_terminal {
                break;
            }
            if ctx.is_cancelled() {
                break;
            }
        }

        let _ = tx.send(Frame::done(ctx.call_id)).await;
        Ok(())
    }
}
