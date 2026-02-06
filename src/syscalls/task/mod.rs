mod list;
mod read;
mod search;

pub use list::TaskList;
pub use read::TaskRead;
pub use search::TaskSearch;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext, TaskKernel, TaskStatus};
use crate::runtime::Kernel;

pub struct TaskEnqueue;

impl TaskEnqueue {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskEnqueue {
    fn name(&self) -> &'static str {
        "task:enqueue"
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

        let task = TaskKernel::task_from_json(data).map_err(KernelError::invalid_args)?;
        let task_id = task.id.clone();
        k.tasks().enqueue(task).await;
        k.bump_activity();

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"task_id": task_id})))
            .await;
        Ok(())
    }
}

pub struct TaskLease;

impl TaskLease {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskLease {
    fn name(&self) -> &'static str {
        "task:lease"
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

        let hand_id = data
            .get("hand_id")
            .and_then(|v| v.as_str())
            .unwrap_or("hand")
            .trim();
        if hand_id.is_empty() {
            return Err(KernelError::invalid_args("hand_id is required"));
        }

        let task = tokio::select! {
            _ = ctx.cancel.cancelled() => {
                return Err(KernelError::cancelled("operation cancelled"));
            }
            t = k.tasks().lease(hand_id) => t,
        };

        k.bump_activity();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "task_id": task.id,
                    "head_id": task.head_id,
                    "scope": task.scope,
                    "goal": task.goal,
                    "input": task.input,
                    "notify_scope": task.notify_scope,
                    "reply_to": task.reply_to.map(|u| u.to_string()),
                }),
            ))
            .await;
        Ok(())
    }
}

pub struct TaskComplete;

impl TaskComplete {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskComplete {
    fn name(&self) -> &'static str {
        "task:complete"
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

        let task_id = data
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err(KernelError::invalid_args("task_id is required"));
        }

        let ok = data.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let summary = data
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        k.tasks().complete(task_id, ok, summary).await;
        k.bump_activity();
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"updated": true})))
            .await;
        Ok(())
    }
}

pub struct TaskStatusGet;

impl TaskStatusGet {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for TaskStatusGet {
    fn name(&self) -> &'static str {
        "task:status"
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

        let task_id = data
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err(KernelError::invalid_args("task_id is required"));
        }

        let status = k.tasks().status(task_id).await;
        let out = match status {
            None => json!({"exists": false}),
            Some(TaskStatus::Queued) => json!({"exists": true, "status": "queued"}),
            Some(TaskStatus::Running { hand_id, .. }) => {
                json!({"exists": true, "status": "running", "hand_id": hand_id})
            }
            Some(TaskStatus::Done { ok, summary, .. }) => {
                json!({"exists": true, "status": "done", "ok": ok, "summary": summary})
            }
        };

        let _ = tx.send(Frame::ok(ctx.call_id, out)).await;
        Ok(())
    }
}

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(TaskEnqueue::new()));
    dispatcher.register(Arc::new(TaskLease::new()));
    dispatcher.register(Arc::new(TaskComplete::new()));
    dispatcher.register(Arc::new(TaskStatusGet::new()));
    dispatcher.register(Arc::new(TaskList::new()));
    dispatcher.register(Arc::new(TaskRead::new()));
    dispatcher.register(Arc::new(TaskSearch::new()));
}
