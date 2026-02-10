//! EMS Syscalls - Entity Management System operations via kernel dispatcher
//!
//! Provides `ems:list`, `ems:insert`, `ems:select`, `ems:update`, `ems:delete`,
//! and `ems:describe` syscalls that route through the standard tool dispatch pipeline.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

// ---------------------------------------------------------------------------
// Shared argument types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListArgs {
    table: String,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct InsertArgs {
    table: String,
    values: Value,
}

#[derive(Debug, Deserialize)]
struct SelectArgs {
    table: String,
    #[serde(rename = "where")]
    where_clause: Option<Value>,
    columns: Option<Vec<String>>,
    order_by: Option<Value>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct UpdateArgs {
    table: String,
    #[serde(rename = "where")]
    where_clause: Value,
    changes: Value,
}

#[derive(Debug, Deserialize)]
struct DeleteArgs {
    table: String,
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DescribeArgs {
    table: Option<String>,
}

// ---------------------------------------------------------------------------
// Helper: acquire EMS handle
// ---------------------------------------------------------------------------

fn get_ems() -> Result<crate::ems::EmsHandle, KernelError> {
    let k = Kernel::get().ok_or_else(|| KernelError::internal("kernel not initialized"))?;
    k.ems()
        .ok_or_else(|| KernelError::internal("EMS not attached"))
}

// ---------------------------------------------------------------------------
// ems:list
// ---------------------------------------------------------------------------

pub struct EmsList;

impl Default for EmsList {
    fn default() -> Self {
        Self
    }
}

impl EmsList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for EmsList {
    fn name(&self) -> &'static str {
        "ems:list"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let args: ListArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let ems = get_ems()?;
        let guard = ems.lock().await;
        let rows = guard
            .select(
                &args.table,
                None,
                None,
                None,
                Some(args.limit.unwrap_or(100)),
                args.offset,
            )
            .await
            .map_err(|e| KernelError::io(e.to_string()))?;

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"rows": rows}))).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ems:insert
// ---------------------------------------------------------------------------

pub struct EmsInsert;

impl Default for EmsInsert {
    fn default() -> Self {
        Self
    }
}

impl EmsInsert {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for EmsInsert {
    fn name(&self) -> &'static str {
        "ems:insert"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;
        let args: InsertArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let ems = get_ems()?;
        let mut guard = ems.lock().await;
        let row = guard
            .insert(&args.table, &args.values)
            .await
            .map_err(|e| KernelError::io(e.to_string()))?;

        let _ = tx.send(Frame::ok(ctx.call_id, row)).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ems:select
// ---------------------------------------------------------------------------

pub struct EmsSelect;

impl Default for EmsSelect {
    fn default() -> Self {
        Self
    }
}

impl EmsSelect {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for EmsSelect {
    fn name(&self) -> &'static str {
        "ems:select"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let args: SelectArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let ems = get_ems()?;
        let guard = ems.lock().await;
        let rows = guard
            .select(
                &args.table,
                args.where_clause.as_ref(),
                args.columns.as_deref(),
                args.order_by.as_ref(),
                Some(args.limit.unwrap_or(100)),
                args.offset,
            )
            .await
            .map_err(|e| KernelError::io(e.to_string()))?;

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"rows": rows}))).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ems:update
// ---------------------------------------------------------------------------

pub struct EmsUpdate;

impl Default for EmsUpdate {
    fn default() -> Self {
        Self
    }
}

impl EmsUpdate {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for EmsUpdate {
    fn name(&self) -> &'static str {
        "ems:update"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;
        let args: UpdateArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let ems = get_ems()?;
        let mut guard = ems.lock().await;
        let n = guard
            .update(&args.table, &args.where_clause, &args.changes)
            .await
            .map_err(|e| KernelError::io(e.to_string()))?;

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"changes": n}))).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ems:delete
// ---------------------------------------------------------------------------

pub struct EmsDelete;

impl Default for EmsDelete {
    fn default() -> Self {
        Self
    }
}

impl EmsDelete {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for EmsDelete {
    fn name(&self) -> &'static str {
        "ems:delete"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        ctx.require_mutation()?;
        let args: DeleteArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let ems = get_ems()?;
        let mut guard = ems.lock().await;
        let n = guard
            .delete(&args.table, &args.ids)
            .await
            .map_err(|e| KernelError::io(e.to_string()))?;

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"changes": n}))).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ems:describe
// ---------------------------------------------------------------------------

pub struct EmsDescribe;

impl Default for EmsDescribe {
    fn default() -> Self {
        Self
    }
}

impl EmsDescribe {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for EmsDescribe {
    fn name(&self) -> &'static str {
        "ems:describe"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let args: DescribeArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let ems = get_ems()?;
        let guard = ems.lock().await;
        let info = guard
            .describe(args.table.as_deref())
            .await
            .map_err(|e| KernelError::io(e.to_string()))?;

        let _ = tx.send(Frame::ok(ctx.call_id, info)).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(EmsList::new()));
    dispatcher.register(Arc::new(EmsInsert::new()));
    dispatcher.register(Arc::new(EmsSelect::new()));
    dispatcher.register(Arc::new(EmsUpdate::new()));
    dispatcher.register(Arc::new(EmsDelete::new()));
    dispatcher.register(Arc::new(EmsDescribe::new()));
}
