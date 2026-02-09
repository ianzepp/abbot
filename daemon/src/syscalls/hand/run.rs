//! Hand:Run - Execute a focused work task via the hand's LLM+tool loop
//!
//! Accepts a prompt and optional context, builds a hand bundle, runs the
//! inner LLM+tool loop, and returns a summary of the work performed.
//! This makes hands directly invocable (e.g., from room agents) without
//! going through the task queue.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::{Kernel, execute_hand_loop};

static HAND_PID: AtomicU64 = AtomicU64::new(0);

pub struct HandRun;

impl Default for HandRun {
    fn default() -> Self {
        Self::new()
    }
}

impl HandRun {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for HandRun {
    fn name(&self) -> &'static str {
        "hand:run"
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

        let prompt = data
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if prompt.is_empty() {
            return Err(KernelError::invalid_args("prompt is required"));
        }

        let context = data
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let max_iters = data
            .get("max_iters")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(24);

        let workspace = k.workspace().to_path_buf();

        let pid = HAND_PID.fetch_add(1, Ordering::Relaxed);
        let actor = format!("hand/{pid}");

        // Emit hand:start event
        let _ = tx
            .send(
                Frame::event(
                    ctx.call_id,
                    json!({
                        "kind": "hand:start",
                        "prompt": &prompt,
                    }),
                )
                .with_actor(actor.clone())
                .with_name("hand:run"),
            )
            .await;

        let result = execute_hand_loop(
            &prompt,
            &context,
            max_iters,
            &workspace,
            &actor,
            ctx.cancel.clone(),
        )
        .await;

        // Emit hand:end event
        let _ = tx
            .send(
                Frame::event(
                    ctx.call_id,
                    json!({
                        "kind": "hand:end",
                        "ok": result.ok,
                        "summary": &result.summary,
                    }),
                )
                .with_actor(actor.clone())
                .with_name("hand:run"),
            )
            .await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "ok": result.ok,
                    "summary": result.summary,
                }),
            ))
            .await;
        Ok(())
    }
}
