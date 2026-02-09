//! Chat - Real-time message exchange and tool coordination between agents and users
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The chat namespace implements bidirectional, real-time communication between users and
//! Abbot agents (head, hand, room). It manages the complete lifecycle of conversational turns:
//! messages, tool calls, tool results, completion signals, errors, and cancellations.
//!
//! **Core responsibilities:**
//! - Emit chat messages from users and "head" agents to subscribers
//! - Coordinate tool calls initiated by agents (chat:tool) and deliver results (chat:tool_result)
//! - Signal turn completion (chat:done) or errors (chat:error) to close conversation streams
//! - Support mid-turn cancellation (chat:cancel) for user-initiated interruptions
//! - Chat history is persisted centrally via the kernel's FrameStore
//!
//! **Integration points:**
//! - `TurnTracker` (kernel service) - Manages turn state, tool call registration, cancellation
//! - `Sigcalls` (kernel service) - Real-time signal broadcast to scope/reply_to subscribers
//! - `need:enqueue` - Dispatches user messages to the Need lane for agent processing
//! - `FrameStore` - Centralized persistence of all frames (automatic via dispatcher)
//!
//! **Frame protocol:**
//! All chat syscalls emit `Frame::item` signals to subscribers via `Sigcalls`, enabling:
//! - Streaming text deltas (chat:message)
//! - Tool call notifications (chat:tool)
//! - Turn completion/error events (chat:done, chat:error)
//!
//! DESIGN PHILOSOPHY
//! =================
//! - **Actor-based messaging**: Only users and "head" agents may send messages (no "hand" agents)
//! - **Scope-based isolation**: Messages are broadcast only to subscribers of `(scope, reply_to)`
//! - **Tool coordination lifecycle**: Register external tool → deliver result → check cancellation
//! - **Signal-first, persistence-automatic**: Real-time events via Sigcalls, frames persisted by dispatcher
//! - **Graceful termination**: chat:done/error/cancel close signal streams to prevent resource leaks
//!
//! REGISTERED SYSCALLS
//! ===================
//! 1. `chat:message` - Send text message from user or head agent
//! 2. `chat:tool` - Register external tool call initiated by agent
//! 3. `chat:tool_result` - Deliver tool execution result to agent
//! 4. `chat:done` - Signal turn completion (complete or awaiting_tools)
//! 5. `chat:error` - Signal turn error and close stream
//! 6. `chat:cancel` - Cancel in-flight turn via TurnTracker
//!
//! CONCURRENCY
//! ===========
//! - **Lane assignment**: Immediate lane for all chat syscalls (user-facing, low latency)
//! - **Signal broadcasting**: Sigcalls uses async broadcast (no blocking send)
//! - **Frame persistence**: Automatic via dispatcher (does not block syscall response)
//! - **Turn state access**: TurnTracker is thread-safe (Arc<Mutex<...>>)
//!
//! SECURITY MODEL
//! ==============
//! - **Actor restrictions**: chat:message enforces user/head-only (no hand agents)
//! - **Scope isolation**: Messages only visible to subscribers of matching (scope, reply_to)
//! - **No filesystem access**: Chat operations are purely in-memory (signals + database logging)
//! - **Cancellation propagation**: chat:cancel triggers TurnTracker cancellation, stopping tasks
//!
//! FRAME LIFECYCLE
//! ===============
//! WHY different frame types for chat operations:
//!
//! 1. **Frame::item** (via Sigcalls.send)
//!    - Broadcasts real-time events to subscribers (text deltas, tool calls, completion)
//!    - Subscriber-side delivery (not response to syscall caller)
//!    - Enables streaming UI updates as conversation progresses
//!
//! 2. **Frame::ok** (returned to syscall caller)
//!    - Confirms syscall execution succeeded (e.g., `{"sent": true}`)
//!    - Caller-side acknowledgment (not broadcast to subscribers)
//!
//! 3. **Frame::done** (via Sigcalls.send for chat:done)
//!    - Signals end of turn stream (subscribers stop listening)
//!    - Follows Frame::item pattern (subscriber-side)
//!
//! WHY separation between caller response and subscriber signals:
//! - Caller needs immediate ack that syscall was accepted
//! - Subscribers need real-time events for UI updates
//! - Decouples syscall execution from event broadcasting

mod cancel;
mod done;
mod error;
mod message;
mod tool;
mod tool_result;

pub use cancel::ChatCancel;
pub use done::ChatDone;
pub use error::ChatError;
pub use message::ChatMessage;
pub use tool::ChatTool;
pub use tool_result::ChatToolResult;

use uuid::Uuid;

use crate::kernel::KernelError;

// =============================================================================
// SHARED PARSING UTILITIES
// =============================================================================
//
// WHY: All chat syscalls require `room` (conversation identifier) and `reply_to`
// (turn identifier). Centralizing parsing ensures consistent validation and error
// messages across the namespace.

/// Parse and validate the `room` field from syscall arguments.
///
/// WHY: Room identifies the conversation (session ID, user ID, etc.) and is
/// used for message routing via Sigcalls. Required for all chat operations.
///
/// SECURITY: Room is a free-form string (no validation beyond non-empty). Caller
/// is responsible for ensuring room uniqueness and access control.
pub(crate) fn parse_room(data: &serde_json::Value) -> Result<&str, KernelError> {
    let room = data
        .get("room")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if room.is_empty() {
        return Err(KernelError::invalid_args("room is required"));
    }
    Ok(room)
}

/// Parse and validate the `reply_to` field as a UUID from syscall arguments.
///
/// WHY: reply_to identifies the specific turn within a conversation. Used for
/// TurnTracker lookups and Sigcalls broadcasting. Required for all chat operations.
///
/// SECURITY: Must be a valid UUID. No validation that the turn exists (caller
/// may reference future or non-existent turns).
pub(crate) fn parse_reply_to(data: &serde_json::Value) -> Result<Uuid, KernelError> {
    let reply_to = data
        .get("reply_to")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if reply_to.is_empty() {
        return Err(KernelError::invalid_args("reply_to is required"));
    }
    Uuid::parse_str(reply_to)
        .map_err(|_| KernelError::invalid_args("reply_to must be a valid UUID"))
}

// =============================================================================
// SYSCALL REGISTRATION
// =============================================================================

/// Register all chat namespace syscalls with the kernel dispatcher.
///
/// WHY: Centralized registration ensures all six chat syscalls are consistently
/// available. Called during kernel initialization (dispatcher setup phase).
///
/// IMPORTANT: Order is arbitrary (dispatcher uses HashMap), but follows logical
/// flow: message → tool → tool_result → done/error/cancel.
pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(ChatMessage));
    dispatcher.register(Arc::new(ChatTool));
    dispatcher.register(Arc::new(ChatToolResult));
    dispatcher.register(Arc::new(ChatDone));
    dispatcher.register(Arc::new(ChatError));
    dispatcher.register(Arc::new(ChatCancel));
}
