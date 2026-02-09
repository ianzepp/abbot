//! Chat:Message - Send text messages from users or head agents
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables users and "head" agents to send text messages within a conversation.
//! It implements **actor-based routing**: user messages are injected directly into rooms
//! via the RoomRegistry, while head messages emit real-time text deltas (streaming agent responses).
//!
//! **Critical design decisions:**
//! - User messages → direct room injection (get-or-create room, attach door, inject message)
//! - Head messages → Sigcalls broadcast (real-time text streaming to subscribers)
//! - "Hand" agents are explicitly forbidden (prevents LLM-controlled message injection)
//!
//! **Integration points:**
//! - `RoomRegistry` - Manages persistent named rooms for interactive chat
//! - `Sigcalls` - Real-time broadcast of head agent text deltas to UI subscribers
//! - `FrameStore` - Centralized persistence of all frames (automatic via dispatcher)
//!
//! **Frame protocol:**
//! - Emits `Frame::item` (type: text_delta) via Sigcalls for head messages
//! - Returns `Frame::ok` to caller with `{"sent": true}` acknowledgment

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::head::config::{
    head_context_budget_tokens, head_time_gap_marker_minutes, load_tars_dials,
};
use crate::runtime::room::door::{Door, WebSocketDoor};
use crate::runtime::{
    AppConfig, HeadBundleBuilder, HeadBundleConfig, Kernel, Room, RoomAgent, RoomRunner, RoomType,
};
use crate::syscalls::dispatch::head_room_catalog;

use super::{parse_reply_to, parse_room};

/// Syscall for sending text messages from users or head agents.
pub struct ChatMessage;

#[async_trait]
impl Syscall for ChatMessage {
    fn name(&self) -> &'static str {
        "chat:message"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Argument Validation
        // =====================================================================
        ctx.check_cancelled()?;

        let room = parse_room(&data)?;
        let reply_to = parse_reply_to(&data)?;
        let content = data
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if content.trim().is_empty() {
            return Err(KernelError::invalid_args("content is required"));
        }

        let actor = ctx.actor_str();

        // =====================================================================
        // PHASE 2: Actor-Based Routing
        // =====================================================================

        if actor.starts_with("head/") {
            // -----------------------------------------------------------------
            // HEAD AGENT PATH: Stream Text Delta via Sigcalls
            // -----------------------------------------------------------------
            let Some(k) = Kernel::get() else {
                return Err(KernelError::internal("kernel not initialized"));
            };

            k.sigcalls()
                .send(
                    room,
                    reply_to,
                    Frame::item(
                        ctx.call_id,
                        json!({"type": "text_delta", "content": content}),
                    )
                    .with_name("chat:message")
                    .with_actor(actor.to_string()),
                )
                .await;
        } else if actor == "user" || actor.starts_with("human/") {
            // -----------------------------------------------------------------
            // USER PATH: Direct room injection via RoomRegistry
            // -----------------------------------------------------------------
            let Some(k) = Kernel::get() else {
                return Err(KernelError::internal("kernel not initialized"));
            };

            if !data
                .get("interactive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                // NON-INTERACTIVE: Persist as chat:user frame for room injection.
                k.sigcalls()
                    .send(
                        room,
                        reply_to,
                        Frame::item(
                            ctx.call_id,
                            json!({
                                "kind": "chat:user",
                                "data": {"content": content, "sender": actor}
                            }),
                        )
                        .with_name("chat:message")
                        .with_actor(actor.to_string()),
                    )
                    .await;
            } else {
                // INTERACTIVE: Get-or-create room, attach door, inject message.
                let store = k
                    .store()
                    .ok_or_else(|| KernelError::internal("store not attached"))?;
                let snapshot = k
                    .snapshot()
                    .ok_or_else(|| KernelError::internal("snapshot not attached"))?;
                let workspace = k.workspace().to_path_buf();

                // Build bundle messages (only used if room is new)
                let rooms = vec![room.to_string()];
                let tars = load_tars_dials(&workspace);
                let traits = AppConfig::global().traits.to_trait_names();
                let bundle_builder = HeadBundleBuilder::new_with_snapshot(
                    store.clone(),
                    workspace.clone(),
                    snapshot.clone(),
                );
                let bundle_cfg = HeadBundleConfig::new("head-0", rooms)
                    .with_context_budget_tokens(head_context_budget_tokens())
                    .with_time_gap_marker_minutes(head_time_gap_marker_minutes())
                    .with_traits(traits)
                    .with_tars(tars);
                let initial_messages = bundle_builder.build(&bundle_cfg).await;

                let system_prompt: String = initial_messages
                    .first()
                    .and_then(|m| m.content.clone())
                    .unwrap_or_default();

                // Build door for this turn
                let snap = snapshot.get();
                let door: Arc<dyn Door> = Arc::new(WebSocketDoor {
                    room: room.to_string(),
                    thread_id: reply_to,
                    actor: "head/head-0".to_string(),
                    workspace: workspace.clone(),
                    external_tool_specs: snap.external_tools.clone(),
                    external_names: snap.external_tool_names.clone(),
                    session_locks: k.session_locks().clone(),
                });

                // Get or create room (closure only runs on first creation)
                let room_owned = room.to_string();
                let room_for_runner = room_owned.clone();
                let _active = k
                    .rooms()
                    .get_or_create(room, move || {
                        let mut agent =
                            RoomAgent::new("head-0", "head", system_prompt, head_room_catalog());
                        agent.messages = initial_messages;

                        let r = Room::new(
                            uuid::Uuid::new_v4().to_string(),
                            room_owned.clone(),
                            RoomType::General,
                            "Interactive chat",
                            vec![agent],
                            12,
                        );

                        let runner = RoomRunner::new(store, room_for_runner.as_str());
                        (r, runner)
                    })
                    .await;

                // Attach door (resets room for new turn) and inject user message
                k.rooms().attach_door(room, door).await;
                k.rooms().inject_message(room, content.clone()).await;

                // Return immediately — room runner processes asynchronously.
                // The ChatHandler's turn stream (opened before this syscall)
                // receives response frames via the Door.
            }
        } else {
            // -----------------------------------------------------------------
            // FORBIDDEN ACTORS: Hand, Room, etc.
            // -----------------------------------------------------------------
            return Err(KernelError::invalid_args(
                "chat:message actor must be user or head/*",
            ));
        }

        // =====================================================================
        // PHASE 3: Acknowledgment
        // =====================================================================
        let _ = tx.send(Frame::ok(ctx.call_id, json!({"sent": true}))).await;
        Ok(())
    }
}
