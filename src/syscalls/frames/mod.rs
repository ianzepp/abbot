use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::frame_select::build_frame_select_sql;
use crate::kernel::{Frame, FrameSelectArgs, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

pub struct FramesAppend;

impl FramesAppend {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for FramesAppend {
    fn name(&self) -> &'static str {
        "frames:append"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;

        let kind = data
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("log")
            .trim();
        if kind.is_empty() {
            return Err(KernelError::invalid_args("kind is required"));
        }

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        let payload = json!({
            "kind": kind,
            "scope": scope,
            "data": data.get("data").cloned().unwrap_or(serde_json::Value::Null),
        });

        let _ = tx.send(Frame::event(ctx.call_id, payload)).await;
        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"appended": true})))
            .await;
        Ok(())
    }
}

pub struct FramesSelect;

impl FramesSelect {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for FramesSelect {
    fn name(&self) -> &'static str {
        "frames:select"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        use rusqlite::{Connection, params_from_iter};

        ctx.check_cancelled()?;

        let args: FrameSelectArgs = serde_json::from_value(data)
            .map_err(|e| KernelError::invalid_args(format!("invalid arguments: {e}")))?;

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let store = k
            .frames()
            .ok_or_else(|| KernelError::internal("frame store not initialized"))?;

        let limit = args.limit.unwrap_or(200).clamp(1, 2000) as i64;
        let include_frame = args.include_frame.unwrap_or(true);
        let include_json = args.include_json.unwrap_or(false);

        let order = match args
            .order
            .as_deref()
            .unwrap_or("asc")
            .to_lowercase()
            .as_str()
        {
            "asc" => "ASC",
            "desc" => "DESC",
            other => {
                return Err(KernelError::invalid_args(format!(
                    "invalid order '{other}' (expected 'asc' or 'desc')"
                )));
            }
        };

        let (sql, params) = build_frame_select_sql(&args, order, limit);

        let (items, max_seq) = {
            let conn = Connection::open(store.db_path())
                .map_err(|e| KernelError::io(format!("frames db open failed: {e}")))?;

            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| KernelError::io(format!("frames query prepare failed: {e}")))?;

            let mut rows = stmt
                .query(params_from_iter(params))
                .map_err(|e| KernelError::io(format!("frames query failed: {e}")))?;

            let mut items: Vec<serde_json::Value> = Vec::new();
            let mut max_seq: u64 = 0;
            while let Some(row) = rows
                .next()
                .map_err(|e| KernelError::io(format!("frames read failed: {e}")))?
            {
                ctx.check_cancelled()?;

                let seq: i64 = row.get(0).unwrap_or(0);
                let ts_ms: i64 = row.get(1).unwrap_or(0);
                let op: String = row.get(2).unwrap_or_default();
                let name: Option<String> = row.get(3).ok();
                let actor: Option<String> = row.get(4).ok();
                let frame_id: String = row.get(5).unwrap_or_default();
                let parent_id: Option<String> = row.get(6).ok();
                let scope: Option<String> = row.get(7).ok();
                let kind: Option<String> = row.get(8).ok();
                let reply_to: Option<String> = row.get(9).ok();
                let frame_json: String = row.get(10).unwrap_or_else(|_| "{}".to_string());

                let seq_u = seq.max(0) as u64;
                max_seq = max_seq.max(seq_u);

                let mut out = json!({
                    "seq": seq_u,
                    "ts_ms": ts_ms,
                    "op": op,
                    "name": name,
                    "actor": actor,
                    "frame_id": frame_id,
                    "parent_id": parent_id,
                    "scope": scope,
                    "kind": kind,
                    "reply_to": reply_to,
                });

                if include_frame {
                    let frame: Frame = serde_json::from_str(&frame_json).unwrap_or_else(|_| {
                        Frame::error(
                            ctx.call_id,
                            json!({"code": "E_LOG_PARSE", "message": "failed to parse frame"}),
                        )
                    });
                    out["frame"] = serde_json::to_value(&frame).unwrap_or(serde_json::Value::Null);
                }
                if include_json {
                    out["frame_json"] = serde_json::Value::String(frame_json);
                }

                items.push(out);
            }

            (items, max_seq)
        };

        let count = items.len() as u64;

        for item in items {
            ctx.check_cancelled()?;
            let _ = tx.send(Frame::item(ctx.call_id, item)).await;
        }

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"count": count, "next_since_seq": max_seq}),
            ))
            .await;
        Ok(())
    }
}

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(FramesAppend::new()));
    dispatcher.register(Arc::new(FramesSelect::new()));
}
