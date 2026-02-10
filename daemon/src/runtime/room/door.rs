//! Door - Bidirectional bridge between an external channel and a room.
//!
//! A Door connects a room to an external client (e.g., TUI, web UI, Slack).
//! When a user connects, a Door is opened on the room. It relays LLM output
//! to the client (via chat:* syscalls) and provides external tool coordination.
//!
//! The `Door` trait defines the protocol-agnostic interface. `WebSocketDoor`
//! implements it for WebSocket-based clients (TUI, web UI).

use std::collections::HashSet;
use std::fmt::Debug;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::hal::llm::ToolSpec;
use crate::kernel::{ExternalToolResult, Frame, TurnKey, TurnWaitError};
use crate::runtime::Kernel;
use crate::runtime::session_locks::{SessionWriteGuard, SessionWriteLocks};

/// Bidirectional bridge between an external channel and a room.
///
/// Protocol-agnostic interface for relaying LLM output to clients,
/// coordinating external tools, and managing session write locks.
#[async_trait]
pub trait Door: Send + Sync + Debug {
    /// Emit a chat message to the client via chat:message syscall.
    async fn emit_chat_message(&self, content: &str) -> Result<(), String>;
    /// Emit an external tool call to the client via chat:tool syscall.
    async fn emit_chat_tool(
        &self,
        tool_call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<(), String>;
    /// Signal turn completion via chat:done syscall.
    async fn emit_chat_done(&self, reason: &str) -> Result<(), String>;
    /// Signal an error to the client via chat:error syscall.
    async fn emit_chat_error(&self, code: &str, message: &str) -> Result<(), String>;
    /// Emit a "thinking" status indicator via chat:status syscall.
    async fn emit_chat_thinking(&self) -> Result<(), String>;
    /// Emit a tool activity status line via chat:status syscall.
    async fn emit_chat_activity(
        &self,
        actor: &str,
        tool: &str,
        summary: &str,
    ) -> Result<(), String>;
    /// Emit an intermediate thought (LLM text between tool calls) via chat:status syscall.
    async fn emit_chat_thought(&self, actor: &str, content: &str) -> Result<(), String>;
    /// Check if the current turn has been cancelled by the client.
    async fn is_turn_cancelled(&self) -> bool;
    /// Check if a tool name is an external (user__*) tool.
    fn is_external_tool(&self, name: &str) -> bool;
    /// Wait for an external tool result from the client.
    async fn wait_for_external_tool_result(
        &self,
        tool_call_id: &str,
    ) -> Result<ExternalToolResult, TurnWaitError>;
    /// Acquire the session write lock for the door's room.
    async fn acquire_write_lock(&self) -> SessionWriteGuard;
    /// External (user__*) tool specs to append to agent tools.
    fn external_tools(&self) -> &[ToolSpec];
}

/// WebSocket-based Door for TUI and web UI clients.
///
/// Relays LLM output via kernel dispatcher (chat:* syscalls) and coordinates
/// external tool calls through the SigcallHub turn system.
#[derive(Clone)]
pub struct WebSocketDoor {
    /// Room name for frame emission (e.g., "main", "session/abc").
    pub room: String,
    /// Reply-to UUID for SigcallHub threading.
    pub thread_id: Uuid,
    /// Actor string for frame emission (e.g., "head/default").
    pub actor: String,
    /// Workspace root for syscall dispatch.
    pub workspace: PathBuf,
    /// External (user__*) tool specs to append to agent tools.
    pub external_tool_specs: Vec<ToolSpec>,
    /// External (user__*) tool name lookup set.
    pub external_names: HashSet<String>,
    /// Session write locks for mutation serialization.
    pub session_locks: SessionWriteLocks,
}

impl Debug for WebSocketDoor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketDoor")
            .field("room", &self.room)
            .field("thread_id", &self.thread_id)
            .field("actor", &self.actor)
            .field("workspace", &self.workspace)
            .field("external_names", &self.external_names)
            .finish()
    }
}

#[async_trait]
impl Door for WebSocketDoor {
    async fn emit_chat_message(&self, content: &str) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:message",
            json!({
                "room": self.room,
                "reply_to": self.thread_id.to_string(),
                "content": content,
            }),
        )
        .with_actor(self.actor.clone());
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let _ = rx.recv().await;
        Ok(())
    }

    async fn emit_chat_tool(
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
                "room": self.room,
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

    async fn emit_chat_done(&self, reason: &str) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:done",
            json!({
                "room": self.room,
                "reply_to": self.thread_id.to_string(),
                "reason": reason,
            }),
        )
        .with_actor(self.actor.clone());
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let _ = rx.recv().await;
        Ok(())
    }

    async fn emit_chat_error(&self, code: &str, message: &str) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:error",
            json!({
                "room": self.room,
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

    async fn emit_chat_thinking(&self) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:status",
            json!({
                "room": self.room,
                "reply_to": self.thread_id.to_string(),
                "status": "thinking",
            }),
        )
        .with_actor(self.actor.clone());
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let _ = rx.recv().await;
        Ok(())
    }

    async fn emit_chat_activity(
        &self,
        actor: &str,
        tool: &str,
        summary: &str,
    ) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:status",
            json!({
                "room": self.room,
                "reply_to": self.thread_id.to_string(),
                "status": "tool",
                "actor": actor,
                "tool": tool,
                "summary": summary,
            }),
        )
        .with_actor(self.actor.clone());
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let _ = rx.recv().await;
        Ok(())
    }

    async fn emit_chat_thought(&self, actor: &str, content: &str) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "chat:status",
            json!({
                "room": self.room,
                "reply_to": self.thread_id.to_string(),
                "status": "thought",
                "actor": actor,
                "content": content,
            }),
        )
        .with_actor(self.actor.clone());
        let mut rx = dispatcher.dispatch(req, self.workspace.clone(), CancellationToken::new());
        let _ = rx.recv().await;
        Ok(())
    }

    async fn is_turn_cancelled(&self) -> bool {
        let Some(k) = Kernel::get() else {
            return false;
        };
        let key = TurnKey::new(&self.room, self.thread_id);
        k.turns().is_cancelled(&key).await
    }

    fn is_external_tool(&self, name: &str) -> bool {
        self.external_names.contains(name)
    }

    async fn wait_for_external_tool_result(
        &self,
        tool_call_id: &str,
    ) -> Result<ExternalToolResult, TurnWaitError> {
        let Some(k) = Kernel::get() else {
            return Err(TurnWaitError::NotFound);
        };
        let key = TurnKey::new(&self.room, self.thread_id);
        k.turns()
            .take_external_tool_result(&key, tool_call_id)
            .await
    }

    async fn acquire_write_lock(&self) -> SessionWriteGuard {
        self.session_locks.acquire(&self.room).await
    }

    fn external_tools(&self) -> &[ToolSpec] {
        &self.external_tool_specs
    }
}
