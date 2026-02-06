//! Room:Run - Execute parallel agent loop for a room
//!
//! Thin wrapper around RoomRunner that integrates with kernel services (store,
//! coordinator, dispatcher) and emits stream events for observability.
//!
//! Builds 3 default agents (MindManager, HeadManager, HandManager) with
//! system prompts and room_catalog tools, then delegates to RoomRunner::run().

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, RoomKind, Syscall, SyscallContext};
use crate::runtime::{Kernel, Room, RoomAgent, RoomConfig, RoomRunner, RoomType};
use crate::syscalls::dispatch::room_catalog;

// =============================================================================
// DEFAULT AGENT SYSTEM PROMPTS
// =============================================================================

const MIND_MANAGER_PROMPT: &str = include_str!("../../runtime/mind_manager.md");
const HEAD_MANAGER_PROMPT: &str = include_str!("../../runtime/head_manager.md");
const HAND_MANAGER_PROMPT: &str = include_str!("../../runtime/hand_manager.md");

/// Build the default 3 agents for a room.
fn build_default_agents() -> Vec<RoomAgent> {
    let tools = room_catalog();
    vec![
        RoomAgent::new("MindManager", "Strategic direction", MIND_MANAGER_PROMPT, tools.clone()),
        RoomAgent::new("HeadManager", "Tactical decisions", HEAD_MANAGER_PROMPT, tools.clone()),
        RoomAgent::new("HandManager", "Operational execution", HAND_MANAGER_PROMPT, tools),
    ]
}

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

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

        let context = data.get("context").and_then(|v| v.as_str()).unwrap_or("");

        // Emit start event
        let _ = k
            .rooms()
            .send(
                room_id,
                Frame::event(
                    room_id,
                    json!({"kind": "room_start", "room_id": room_id.to_string(), "type": rec.kind.as_str(), "context": context}),
                ),
            )
            .await;

        // Construct room and runner
        let scopes = vec![crate::Scope::from(rec.scope.as_str())];
        let room_id_str = room_id.to_string();
        let room_cfg = RoomConfig::from_config();

        let room_type = match rec.kind {
            RoomKind::Conclave => RoomType::Conclave,
            RoomKind::Autonomy => RoomType::Autonomy,
            RoomKind::Work => RoomType::Work,
        };

        let max_rounds = match room_type {
            RoomType::Conclave => room_cfg.max_rounds_conclave,
            RoomType::Autonomy => room_cfg.max_rounds_autonomy,
            RoomType::Work => room_cfg.max_rounds_work,
        };

        let agents = build_default_agents();
        let mut room = Room::new(&room_id_str, room_type, context, agents, max_rounds);

        let runner = RoomRunner::new(store.clone(), scopes, k.workspace().to_path_buf(), room_cfg);
        let summary = runner.run(&mut room, None).await;

        // Emit end event
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
                        "status": if summary.is_some() { "done" } else { "no_summary" },
                        "summary": summary,
                        "transcript": record.as_ref().and_then(|r| serde_json::from_str::<serde_json::Value>(&r.transcript).ok()),
                    }),
                ),
            )
            .await;

        // Close stream
        let _ = k
            .rooms()
            .send(room_id, Frame::ok(room_id, json!({"status": "closed"})))
            .await;
        k.rooms().close_stream(room_id).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({"room_id": room_id.to_string(), "status": "done", "summary": summary}),
            ))
            .await;
        Ok(())
    }
}
