//! Room Syscalls - Conclave and autonomy session management
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `room:*` syscalls manage isolated decision-making sessions (conclaves and
//! autonomy). Rooms are ephemeral contexts where a subset of agents deliberate
//! on a specific question and produce a decision or recommendation. Unlike the
//! main chat turn flow, rooms have their own streaming output and lifecycle.
//!
//! WHY rooms exist: The syscall refactor cleanly separated turn management
//! (`chat:*`) from deliberation sessions (`room:*`). This prevents interference
//! between ongoing user chat and internal agent deliberation, and enables parallel
//! decision-making without polluting the main turn stream.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Stream-then-run: Callers must open `room:stream` before `room:run` to
//!   ensure output is never dropped. This enforces correct ordering at API level.
//! - Event-based output: Room events (`room_start`, `room_end`) are emitted to
//!   the room stream, not the turn stream, isolating deliberation from chat.
//! - Decision persistence: Room transcripts and decisions are stored in the
//!   kernel store for observability and audit.
//!
//! TRADE-OFFS
//! ==========
//! - `room:run` is synchronous (blocks until deliberation completes). This
//!   simplifies caller logic but means long-running deliberations hold the
//!   syscall context. Acceptable because rooms are intentionally bounded tasks.
//! - Room streams are closed automatically by `room:run` after completion.
//!   Callers cannot reuse the same room_id for multiple deliberations.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::kernel::{Frame, FrameOp, KernelError, RoomKind, Syscall, SyscallContext};
use crate::runtime::{Kernel, Room, RoomConfig, RoomRunner, WakeMode};

// =============================================================================
// ROOM:CREATE - Room instantiation
// =============================================================================
//
// WHY this exists: Allocates a new room (conclave or autonomy) with a unique ID.
// The room exists as metadata only until `room:run` is invoked.

pub struct RoomCreate;

impl RoomCreate {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomCreate {
    fn name(&self) -> &'static str {
        "room:create"
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

        let kind = data
            .get("type")
            .or_else(|| data.get("kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let Some(kind) = RoomKind::from_str(kind) else {
            return Err(KernelError::invalid_args(
                "room type must be 'conclave', 'autonomy', or 'work'",
            ));
        };

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();
        if scope.is_empty() {
            return Err(KernelError::invalid_args("scope is required"));
        }

        let room_id = k.rooms().create(kind, scope).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"room_id": room_id.to_string(), "type": kind.as_str(), "scope": scope}),
            ))
            .await;
        Ok(())
    }
}

// =============================================================================
// ROOM:STREAM - Room output stream subscription
// =============================================================================
//
// WHY this exists: Opens a streaming channel for room events before `room:run`
// executes. Enforcing this ordering prevents race conditions where deliberation
// begins before the caller is ready to receive output.
//
// SECURITY NOTE: Stream frames are re-parented to the syscall call_id to prevent
// confusion about frame origin.

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
            // WHY re-parent: Frames emitted by the room must indicate they came from
            // this syscall invocation, not from some internal source.
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

// =============================================================================
// ROOM:RUN - Room deliberation execution
// =============================================================================
//
// WHY this exists: Executes the deliberation (conclave or autonomy) for a room.
// Blocks until completion, emits events to the room stream, and returns the
// final decision.
//
// TRADE-OFF: Synchronous execution simplifies caller logic (no need to poll or
// wait separately) but means long deliberations hold resources. This is acceptable
// because rooms are bounded tasks with clear termination.

pub struct RoomRun;

impl RoomRun {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomRun {
    fn name(&self) -> &'static str {
        "room:run"
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

        let rec = k
            .rooms()
            .get(room_id)
            .await
            .ok_or_else(|| KernelError::not_found("room not found"))?;

        if !k.rooms().has_stream(room_id).await {
            return Err(KernelError::invalid_args(
                "room stream not opened (call room:stream before room:run)",
            ));
        }

        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        let wake_mode = data
            .get("wake_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("normal")
            .trim();
        let wake_mode = match wake_mode {
            "init" => WakeMode::Init,
            _ => WakeMode::Normal,
        };

        let context = data.get("context").and_then(|v| v.as_str()).unwrap_or("");

        // -------------------------------------------------------------------------
        // DELIBERATION: Emit start event, execute deliberation, emit end event
        // WHY this structure: Start/end events enable clients to track deliberation
        // progress and correlate transcript data with room execution.
        // -------------------------------------------------------------------------
        let _ = k
            .rooms()
            .send(
                room_id,
                Frame::event(
                    room_id,
                    json!({"kind": "room_start", "room_id": room_id.to_string(), "type": rec.kind.as_str(), "context": context, "wake_mode": format!("{:?}", wake_mode)}),
                ),
            )
            .await;

        let scopes = vec![crate::Scope::from(rec.scope.as_str())];
        let room_id_str = room_id.to_string();
        let room_cfg = RoomConfig::from_config();
        let runner = RoomRunner::new(store.clone(), scopes, k.workspace().to_path_buf(), room_cfg);

        let mut room = match rec.kind {
            RoomKind::Conclave => Room::conclave(&room_id_str),
            RoomKind::Autonomy => Room::autonomy(&room_id_str),
            RoomKind::Work => Room::work(&room_id_str),
        };

        let decision = runner.run(&mut room, None).await;

        let record = store.get_conclave(&room_id_str).ok().flatten();
        let _ = k
            .rooms()
            .send(
                room_id,
                Frame::event(
                    room_id,
                    json!({
                        "kind": "room_end",
                        "room_id": room_id_str,
                        "status": record.as_ref().map(|r| r.status.clone()).unwrap_or_else(|| if decision.is_some() {"done".to_string()} else {"no_decision".to_string()}),
                        "decision": decision,
                        "transcript": record.as_ref().and_then(|r| serde_json::from_str::<serde_json::Value>(&r.transcript).ok()),
                    }),
                ),
            )
            .await;

        let _ = k
            .rooms()
            .send(room_id, Frame::ok(room_id, json!({"status": "closed"})))
            .await;
        k.rooms().close_stream(room_id).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"room_id": room_id.to_string(), "status": "done", "decision": decision}),
            ))
            .await;
        Ok(())
    }
}

// =============================================================================
// ROOM:SCHEDULE - Persistent room scheduling
// =============================================================================
//
// WHY this exists: Creates a persistent schedule entry for a room to be executed
// at a future time. The coordinator claims and executes due schedules.

pub struct RoomSchedule;

impl RoomSchedule {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomSchedule {
    fn name(&self) -> &'static str {
        "room:schedule"
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
        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        let room_type = data
            .get("room_type")
            .or_else(|| data.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if RoomKind::from_str(room_type).is_none() {
            return Err(KernelError::invalid_args(
                "room_type must be 'conclave', 'autonomy', or 'work'",
            ));
        }

        let scope = data
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .trim();

        let run_after_ms = data
            .get("run_after_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64
            });

        let reason = data
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("scheduled")
            .trim();

        let wake_mode = data
            .get("wake_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("normal")
            .trim();

        let constraints_json = data
            .get("constraints")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "{}".to_string());

        let context = data
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let id = Uuid::new_v4().to_string();

        store
            .insert_room_schedule(
                &id,
                room_type,
                scope,
                run_after_ms,
                reason,
                wake_mode,
                &constraints_json,
                context,
            )
            .map_err(|e| KernelError::internal(format!("failed to insert schedule: {}", e)))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"schedule_id": id, "room_type": room_type, "scope": scope, "run_after_ms": run_after_ms}),
            ))
            .await;
        Ok(())
    }
}

// =============================================================================
// ROOM:LIST - List room schedules
// =============================================================================

pub struct RoomList;

impl RoomList {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomList {
    fn name(&self) -> &'static str {
        "room:list"
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
        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        let status = data.get("status").and_then(|v| v.as_str());
        let room_type = data
            .get("room_type")
            .or_else(|| data.get("type"))
            .and_then(|v| v.as_str());
        let limit = data
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;

        let schedules = store
            .list_room_schedules(status, room_type, limit)
            .map_err(|e| KernelError::internal(format!("failed to list schedules: {}", e)))?;

        let items: Vec<serde_json::Value> = schedules
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "room_type": s.room_type,
                    "scope": s.scope,
                    "status": s.status,
                    "run_after_ms": s.run_after_ms,
                    "reason": s.reason,
                    "wake_mode": s.wake_mode,
                    "context": s.context,
                    "attempts": s.attempts,
                    "last_error": s.last_error,
                    "created_at_ms": s.created_at_ms,
                })
            })
            .collect();

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"schedules": items, "count": items.len()}),
            ))
            .await;
        Ok(())
    }
}

// =============================================================================
// ROOM:RESCHEDULE - Reschedule a room schedule
// =============================================================================

pub struct RoomReschedule;

impl RoomReschedule {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomReschedule {
    fn name(&self) -> &'static str {
        "room:reschedule"
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
        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        let id = data
            .get("id")
            .or_else(|| data.get("schedule_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::invalid_args("id is required"))?;

        let run_after_ms = data
            .get("run_after_ms")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| KernelError::invalid_args("run_after_ms is required"))?;

        let updated = store
            .reschedule_room_schedule(id, run_after_ms)
            .map_err(|e| KernelError::internal(format!("failed to reschedule: {}", e)))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"id": id, "updated": updated, "run_after_ms": run_after_ms}),
            ))
            .await;
        Ok(())
    }
}

// =============================================================================
// ROOM:CANCEL - Cancel a room schedule
// =============================================================================

pub struct RoomCancel;

impl RoomCancel {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Syscall for RoomCancel {
    fn name(&self) -> &'static str {
        "room:cancel"
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
        let store = k
            .store()
            .ok_or_else(|| KernelError::internal("kernel store not attached"))?;

        let id = data
            .get("id")
            .or_else(|| data.get("schedule_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::invalid_args("id is required"))?;

        let cancelled = store
            .cancel_room_schedule(id)
            .map_err(|e| KernelError::internal(format!("failed to cancel: {}", e)))?;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"id": id, "cancelled": cancelled}),
            ))
            .await;
        Ok(())
    }
}

// =============================================================================
// REGISTRATION
// =============================================================================

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    dispatcher.register(Arc::new(RoomCreate::new()));
    dispatcher.register(Arc::new(RoomStream::new()));
    dispatcher.register(Arc::new(RoomRun::new()));
    dispatcher.register(Arc::new(RoomSchedule::new()));
    dispatcher.register(Arc::new(RoomList::new()));
    dispatcher.register(Arc::new(RoomReschedule::new()));
    dispatcher.register(Arc::new(RoomCancel::new()));
}
