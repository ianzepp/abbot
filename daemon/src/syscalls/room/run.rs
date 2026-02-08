//! Room:Run - Execute parallel agent loop for a room
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! Accepts agents from the caller (as a JSON array), generates a room_id internally
//! (UUID v4), and delegates to RoomRunner for parallel multi-agent execution.
//! Returns the room summary in Frame::ok.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Caller-defined agents**: The caller provides agent definitions (name, role,
//!   system_prompt) rather than hardcoding a fixed set. This makes rooms a generic
//!   execution primitive that any syscall or agent can invoke.
//! - **Sensible defaults**: room_type defaults to "general", max_rounds to config
//!   default (10), worktree to false unless room_type is "work".
//! - **Room catalog tools**: All agents receive the room_catalog() tool set by default
//!   (noop/signal, noop/done, plus mind strategic tools).

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::{Kernel, Room, RoomAgent, RoomConfig, RoomRunner, RoomType};
use crate::syscalls::dispatch::room_catalog;

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Execute a parallel multi-agent room session.
///
/// WHY: Provides the core room execution primitive. Unlike the old room:create →
/// room:stream → room:run ceremony, this single syscall handles the full lifecycle:
/// generate room_id, parse agents, run rounds, return summary.
pub struct RoomRun;

impl Default for RoomRun {
    fn default() -> Self {
        Self::new()
    }
}

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

    /// Execute a room with caller-provided agents.
    ///
    /// ARGUMENTS:
    /// - `prompt`: Purpose description for the room session (default: "room session")
    /// - `agents`: Required JSON array, each with `name`, optional `role` and `system_prompt`
    /// - `room_type`: "general" or "work" (default: "general")
    /// - `max_rounds`: Override default round limit (default: from RoomConfig)
    /// - `worktree`: Override worktree provisioning (default: true for "work" rooms)
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{room_id, status, summary}` on completion
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

        // -------------------------------------------------------------------------
        // PHASE 1: PARSE PARAMETERS
        // -------------------------------------------------------------------------
        let room_id = Uuid::new_v4().to_string();

        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&room_id);
        let scope = format!("room/{}", name);

        let prompt = data
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("room session");

        let room_type_str = data
            .get("room_type")
            .and_then(|v| v.as_str())
            .unwrap_or("general");
        let room_type = RoomType::from_str(room_type_str).unwrap_or(RoomType::General);

        let room_cfg = RoomConfig::from_config();
        let max_rounds = data
            .get("max_rounds")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(room_cfg.max_rounds);

        // WHY default worktree from room_type: Work rooms need filesystem isolation
        // by convention, but callers can override for special cases.
        let worktree = data
            .get("worktree")
            .and_then(|v| v.as_bool())
            .unwrap_or(room_type == RoomType::Work);

        // -------------------------------------------------------------------------
        // PHASE 2: BUILD AGENTS
        // WHY agents are required: Rooms are generic execution primitives — the
        // caller decides what agents participate and what their roles are.
        // -------------------------------------------------------------------------
        let agents = if let Some(agents_json) = data.get("agents").and_then(|v| v.as_array()) {
            parse_agents(agents_json)?
        } else {
            return Err(KernelError::invalid_args(
                "agents array is required (each with name, role, system_prompt)",
            ));
        };

        // -------------------------------------------------------------------------
        // PHASE 3: EXECUTE ROOM
        // -------------------------------------------------------------------------
        let mut room = Room::new(&room_id, name, room_type, prompt, agents, max_rounds);
        room.worktree = worktree;

        let runner = RoomRunner::new(store.clone(), &scope);
        let summary = runner.run(&mut room, None).await;

        let _ = tx
            .send(Frame::ok(
                ctx.call_id,
                json!({
                    "room_id": room_id,
                    "status": "done",
                    "summary": summary,
                }),
            ))
            .await;
        Ok(())
    }
}

// =============================================================================
// HELPERS
// =============================================================================

/// Parse agents from a JSON array into RoomAgent instances.
///
/// WHY: Separates JSON parsing from syscall logic for clarity. Each agent gets
/// the default room_catalog() tools — custom per-agent tool sets are not yet
/// supported but the structure allows for it.
fn parse_agents(agents_json: &[serde_json::Value]) -> Result<Vec<RoomAgent>, KernelError> {
    let default_tools = room_catalog();
    let mut agents = Vec::new();

    for (i, entry) in agents_json.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::invalid_args(format!("agents[{}].name is required", i)))?;
        let role = entry
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("participant");
        let system_prompt = entry
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // WHY clone default_tools: Custom per-agent tools not yet supported.
        // All agents share the same room catalog for now.
        let tools = if let Some(_tools_arr) = entry.get("tools").and_then(|v| v.as_array()) {
            default_tools.clone()
        } else {
            default_tools.clone()
        };

        agents.push(RoomAgent::new(name, role, system_prompt, tools));
    }

    Ok(agents)
}
