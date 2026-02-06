//! Room:Request - Composite syscall: create -> stream -> run in single call
//!
//! Convenience wrapper that orchestrates the full room lifecycle in a single
//! call. Hardcodes type=conclave, scope=main, wake_mode=normal.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::kernel::{Frame, FrameOp, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

#[derive(Deserialize)]
struct Args {
    reason: String,
}

pub struct RoomRequest;

impl Default for RoomRequest {
    fn default() -> Self {
        Self::new()
    }
}

impl RoomRequest {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomRequest {
    fn name(&self) -> &'static str {
        "room:request"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let args: Args = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid args: {e}")))?;

        if args.reason.trim().is_empty() {
            return Err(KernelError::invalid_args("reason is empty"));
        }

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };

        let dispatcher = k.dispatcher().await;
        let actor = ctx.actor.as_deref().unwrap_or("system").to_string();

        // Phase 1: Create room
        let create_req = Frame::req("room:create", json!({"type": "conclave", "scope": "main"}))
            .with_actor(actor.clone());
        let mut rx = dispatcher.dispatch(create_req, ctx.cwd.clone(), CancellationToken::new());

        let mut room_id = String::new();
        while let Some(frame) = rx.recv().await {
            if frame.op == FrameOp::Ok {
                if let Some(data) = &frame.data
                    && let Some(id) = data.get("room_id").and_then(|v| v.as_str())
                {
                    room_id = id.to_string();
                }
                break;
            }
            if matches!(frame.op, FrameOp::Error | FrameOp::Done) {
                break;
            }
        }

        if room_id.is_empty() {
            return Err(KernelError::internal("failed to create room"));
        }

        // Phase 2: Open stream (background)
        let stream_cancel = CancellationToken::new();
        let _stream_rx = dispatcher.dispatch(
            Frame::req("room:stream", json!({"room_id": room_id})).with_actor(actor.clone()),
            ctx.cwd.clone(),
            stream_cancel.clone(),
        );

        // Phase 3: Run room
        let run_req = Frame::req(
            "room:run",
            json!({"room_id": room_id, "wake_mode": "normal", "context": args.reason}),
        )
        .with_actor(actor);
        let mut run_rx = dispatcher.dispatch(run_req, ctx.cwd.clone(), CancellationToken::new());

        let mut result = json!({"requested": true, "reason": args.reason});
        while let Some(frame) = run_rx.recv().await {
            if frame.op == FrameOp::Ok {
                result = frame.data.unwrap_or(json!({"requested": true}));
                break;
            }
            if matches!(frame.op, FrameOp::Error | FrameOp::Done) {
                break;
            }
        }

        stream_cancel.cancel();

        let _ = tx.send(Frame::ok(ctx.call_id, result)).await;
        Ok(())
    }
}
