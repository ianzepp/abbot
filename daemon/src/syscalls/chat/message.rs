//! Chat:Message - Send text messages from users or head agents
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This syscall enables users and "head" agents to send text messages within a conversation.
//! It implements **actor-based routing**: user messages trigger need:enqueue (to process the
//! request), while head messages emit real-time text deltas (streaming agent responses).
//!
//! **Critical design decisions:**
//! - User messages → need:enqueue (dispatched to Need lane for agent processing)
//! - Head messages → Sigcalls broadcast (real-time text streaming to subscribers)
//! - Both paths log to history (chat:user or chat:head kind)
//! - "Hand" agents are explicitly forbidden (prevents LLM-controlled message injection)
//!
//! **Integration points:**
//! - `Sigcalls` - Real-time broadcast of head agent text deltas to UI subscribers
//! - `need:enqueue` - Queues user messages for agent processing in Need lane
//! - `FrameStore` - Centralized persistence of all frames (automatic via dispatcher)
//!
//! **Frame protocol:**
//! - Emits `Frame::item` (type: text_delta) via Sigcalls for head messages
//! - Returns `Frame::ok` to caller with `{"sent": true}` acknowledgment
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Actor enforcement**: Only users and head agents may send messages (mutation-like operation)
//! - **Divergent paths**: User messages trigger agent work; head messages stream responses
//! - **Real-time first**: Sigcalls broadcast before logging (prioritize low latency)
//! - **Automatic persistence**: Frame logging handled centrally by dispatcher
//!
//! CONCURRENCY
//! ===========
//! - **Lane assignment**: Immediate lane (user-facing, low latency required)
//! - **User message path**: Synchronous need:enqueue dispatch (blocks until queued)
//! - **Head message path**: Async Sigcalls broadcast (no blocking send)
//! - **Logging**: Automatic via dispatcher FrameStore (does not block syscall response)
//!
//! SECURITY MODEL
//! ==============
//! - **Actor restrictions**: Only user and head/* actors accepted (no hand agents)
//! - **No mutation permission check**: Chat messages are not filesystem mutations
//! - **Content validation**: Non-empty content required (no whitespace-only messages)
//! - **Scope isolation**: Messages broadcast only to (scope, reply_to) subscribers
//!
//! ACTOR BEHAVIOR
//! ==============
//! WHY different handling for user vs. head actors:
//!
//! 1. **User/Human messages**:
//!    - Logged as "chat:user" kind
//!    - Dispatched to need:enqueue (creates task for agent to respond)
//!    - NOT broadcast via Sigcalls (only agent responses stream to UI)
//!
//! 2. **Head agent messages**:
//!    - Logged as "chat:head" kind
//!    - Broadcast via Sigcalls (real-time text delta for streaming UI)
//!    - NOT dispatched to need:enqueue (head is responding, not requesting)
//!
//! WHY hand agents forbidden:
//! - Hand agents execute tool calls from LLMs (untrusted)
//! - Allowing hand to send messages enables LLM-controlled chat injection
//! - Head agents are under Abbot control (trusted decision-making)

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError, Syscall, SyscallContext};
use crate::runtime::Kernel;

use super::{parse_reply_to, parse_scope};

// =============================================================================
// SYSCALL IMPLEMENTATION
// =============================================================================

/// Syscall for sending text messages from users or head agents.
///
/// WHY: Zero-sized struct (stateless). All logic is in execute() method.
/// Enables actor-based routing (user → need:enqueue, head → Sigcalls).
pub struct ChatMessage;

#[async_trait]
impl Syscall for ChatMessage {
    fn name(&self) -> &'static str {
        "chat:message"
    }

    /// Send a text message from user or head agent.
    ///
    /// WHY: Enables bidirectional conversation flow:
    /// - Users send messages to initiate agent work (via need:enqueue)
    /// - Head agents send messages to stream responses (via Sigcalls)
    ///
    /// USE CASE:
    /// - User types "fix the bug in auth.rs" → need:enqueue dispatches to Need lane
    /// - Head agent responds "I found the issue..." → Sigcalls streams text to UI
    ///
    /// SECURITY NOTE: Only user and head/* actors permitted. Hand agents (LLM-controlled)
    /// are forbidden to prevent chat injection attacks.
    ///
    /// RETURNS:
    /// - `Frame::ok` with `{"sent": true}` on successful message delivery
    /// - `E_INVALID_ARGS` if content is empty or actor is unauthorized
    /// - `E_CANCELLED` if context is cancelled mid-execution
    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        // =====================================================================
        // PHASE 1: Argument Validation
        // =====================================================================
        // WHY: Validate scope, reply_to, and content before actor-specific routing.
        // Early cancellation check prevents wasted work on cancelled contexts.
        ctx.check_cancelled()?;

        let scope = parse_scope(&data)?;
        let reply_to = parse_reply_to(&data)?;
        let content = data
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // WHY: Reject empty/whitespace-only messages (prevents spam, UI clutter)
        if content.trim().is_empty() {
            return Err(KernelError::invalid_args("content is required"));
        }

        let actor = ctx.actor_str();

        // =====================================================================
        // PHASE 2: Actor-Based Routing
        // =====================================================================
        // WHY: Different message paths for user vs. head agents. User messages
        // create work for agents (need:enqueue), head messages stream responses.

        if actor.starts_with("head/") {
            // -----------------------------------------------------------------
            // HEAD AGENT PATH: Stream Text Delta via Sigcalls
            // -----------------------------------------------------------------
            // WHY: Head agents respond to user requests by streaming text.
            // Sigcalls broadcasts enable real-time UI updates as agent types.
            let Some(k) = Kernel::get() else {
                return Err(KernelError::internal("kernel not initialized"));
            };

            // WHY: Frame::item with type "text_delta" follows streaming text protocol.
            // Subscribers (UI clients) accumulate deltas to build full message.
            k.sigcalls()
                .send(
                    scope,
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
            // USER PATH: Dispatch to Need Lane via need:enqueue
            // -----------------------------------------------------------------
            // WHY: User messages represent requests for agent work. need:enqueue
            // creates a task in the Need lane, where head agent decides how to respond.
            let Some(k) = Kernel::get() else {
                return Err(KernelError::internal("kernel not initialized"));
            };
            let dispatcher = k.dispatcher().await;

            // WHY: Generate new need_id for each user message. Enables tracking
            // individual requests through the system (debugging, metrics).
            let need_id = Uuid::new_v4().to_string();

            let req = Frame::req(
                "need:enqueue",
                json!({
                    "need_id": need_id,
                    "source": "user",
                    "priority": "normal",
                    "need": content,
                    "context": "",
                    "scope": scope,
                    "reply_to": reply_to.to_string(),
                    "reconvene": false,
                }),
            )
            .with_actor(actor.to_string());

            // WHY: Synchronous dispatch ensures message is queued before returning.
            // Fire-and-forget recv() confirms need:enqueue processed (but doesn't
            // check result - need processing happens asynchronously in Need lane).
            let mut rx = dispatcher.dispatch(
                req,
                k.workspace().to_path_buf(),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx.recv().await;
        } else {
            // -----------------------------------------------------------------
            // FORBIDDEN ACTORS: Hand, Room, etc.
            // -----------------------------------------------------------------
            // WHY: Hand agents are LLM-controlled (untrusted). Allowing them to
            // send chat messages enables injection attacks (e.g., "The user said
            // to delete everything"). Room agents are for reflection (no direct chat).
            return Err(KernelError::invalid_args(
                "chat:message actor must be user or head/*",
            ));
        }

        // =====================================================================
        // PHASE 3: Acknowledgment
        // =====================================================================
        // WHY: Return Frame::ok to caller (not subscribers). Confirms message
        // was accepted and logged, regardless of actor path taken.
        let _ = tx.send(Frame::ok(ctx.call_id, json!({"sent": true}))).await;
        Ok(())
    }
}
