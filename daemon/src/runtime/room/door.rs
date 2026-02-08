//! Door - Bidirectional bridge between an external channel and a room.
//!
//! A Door connects a room to an external client (e.g., TUI, web UI, Slack).
//! When a user connects, a Door is opened on the room. It relays LLM output
//! to the client (via chat:* syscalls) and provides external tool coordination.
//!
//! Future: SlackDoor, DiscordDoor, etc. For now, Door is a concrete struct
//! — extract a trait when the second door type arrives.

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::hal::llm::ToolSpec;
use crate::kernel::{ExternalToolResult, Frame, TurnKey, TurnWaitError};
use crate::runtime::Kernel;
use crate::runtime::session_locks::SessionWriteLocks;

/// Bidirectional bridge between an external channel and a room.
///
/// Opened when a user connects, holds the connection, relays messages in/out
/// (including external tools), closes when done.
#[derive(Clone)]
pub struct Door {
    /// Scope for frame emission (e.g., "main", "session/abc").
    pub scope: String,
    /// Reply-to UUID for SigcallHub threading.
    pub thread_id: Uuid,
    /// Actor string for frame emission (e.g., "head/default").
    pub actor: String,
    /// Workspace root for syscall dispatch.
    pub workspace: PathBuf,
    /// External (user__*) tool specs to append to agent tools.
    pub external_tools: Vec<ToolSpec>,
    /// External (user__*) tool name lookup set.
    pub external_names: HashSet<String>,
    /// Session write locks for mutation serialization.
    pub session_locks: SessionWriteLocks,
}

impl std::fmt::Debug for Door {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Door")
            .field("scope", &self.scope)
            .field("thread_id", &self.thread_id)
            .field("actor", &self.actor)
            .field("workspace", &self.workspace)
            .field("external_names", &self.external_names)
            .finish()
    }
}

impl Door {
    /// Emit a chat message to the client via chat:message syscall.
    pub async fn emit_chat_message(&self, content: &str) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:message",
            json!({
                "scope": self.scope,
                "reply_to": self.thread_id.to_string(),
                "content": content,
            }),
        )
        .with_actor(self.actor.clone());
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let _ = rx.recv().await;
        Ok(())
    }

    /// Emit an external tool call to the client via chat:tool syscall.
    pub async fn emit_chat_tool(
        &self,
        tool_call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:tool",
            json!({
                "scope": self.scope,
                "reply_to": self.thread_id.to_string(),
                "tool_call_id": tool_call_id,
                "name": name,
                "arguments": arguments,
            }),
        )
        .with_actor(self.actor.clone());
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let _ = rx.recv().await;
        Ok(())
    }

    /// Signal turn completion via chat:done syscall.
    pub async fn emit_chat_done(&self, reason: &str) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:done",
            json!({
                "scope": self.scope,
                "reply_to": self.thread_id.to_string(),
                "reason": reason,
            }),
        )
        .with_actor(self.actor.clone());
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let _ = rx.recv().await;
        Ok(())
    }

    /// Signal an error to the client via chat:error syscall.
    pub async fn emit_chat_error(&self, code: &str, message: &str) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:error",
            json!({
                "scope": self.scope,
                "reply_to": self.thread_id.to_string(),
                "code": code,
                "message": message,
            }),
        )
        .with_actor(self.actor.clone());
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let _ = rx.recv().await;
        Ok(())
    }

    /// Check if the current turn has been cancelled by the client.
    pub async fn is_turn_cancelled(&self) -> bool {
        let Some(k) = Kernel::get() else {
            return false;
        };
        let key = TurnKey::new(&self.scope, self.thread_id);
        k.turns().is_cancelled(&key).await
    }

    /// Check if a tool name is an external (user__*) tool.
    pub fn is_external_tool(&self, name: &str) -> bool {
        self.external_names.contains(name)
    }

    /// Wait for an external tool result from the client.
    /// Blocks until the client submits the result via chat:tool_result.
    pub async fn wait_for_external_tool_result(
        &self,
        tool_call_id: &str,
    ) -> Result<ExternalToolResult, TurnWaitError> {
        let Some(k) = Kernel::get() else {
            return Err(TurnWaitError::NotFound);
        };
        let key = TurnKey::new(&self.scope, self.thread_id);
        k.turns()
            .take_external_tool_result(&key, tool_call_id)
            .await
    }

    /// Acquire the session write lock for the door's scope.
    pub async fn acquire_write_lock(&self) -> crate::runtime::session_locks::SessionWriteGuard {
        self.session_locks.acquire(&self.scope).await
    }
}
