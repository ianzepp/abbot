//! Chat Syscalls - User-visible turn lifecycle management
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! The `chat:*` syscalls implement the turn lifecycle from the syscall refactor spec.
//! They separate user-visible operations (messages, tool calls, completion) from
//! internal LLM operations and eliminate the legacy redirect mechanism.
//!
//! WHY this separation: Prior to the refactor, `Frame.name` and `data.kind` had
//! inconsistent semantics, authorship was buried in payloads, and tool flow was
//! limited to a single tool call per turn. The `chat:*` namespace establishes a
//! clean contract: all user-visible output flows through these syscalls, internal
//! operations stay internal, and multiple external tools can be batched before
//! segment completion.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Actor-driven semantics: `chat:message` behavior depends on `actor` field
//!   (user -> log + enqueue, head -> log + emit to turn stream)
//! - Side-effecting emitters: `chat:*` syscalls emit frames to the turn stream
//!   keyed by `(scope, reply_to)` and return simple acknowledgments
//! - Turn segment lifecycle: `chat:done` or `chat:error` terminates a segment,
//!   but the turn may continue if awaiting external tool results
//! - Rendezvous mechanism: `chat:tool` registers pending tool calls;
//!   `chat:tool_result` resolves them and wakes the blocked head
//!
//! TRADE-OFFS
//! ==========
//! - `chat:message` remains a single syscall keyed by actor rather than split
//!   into `chat:ingress` / `chat:emit`. This reduces API surface but requires
//!   runtime validation of actor prefixes.
//! - Tool result correlation is strict: unknown `tool_call_id` returns error
//!   rather than silently dropping. This catches client bugs but requires
//!   careful cleanup on cancellation.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelDispatcher, KernelError, Syscall, SyscallContext, TurnKey};
use crate::runtime::Kernel;

// =============================================================================
// VALIDATION HELPERS
// =============================================================================
//
// WHY separate functions: Common validation logic used across multiple syscalls.
// Centralizing prevents divergence in error messages and validation behavior.

fn parse_scope(data: &serde_json::Value) -> Result<&str, KernelError> {
    let scope = data
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if scope.is_empty() {
        return Err(KernelError::invalid_args("scope is required"));
    }
    Ok(scope)
}

fn parse_reply_to(data: &serde_json::Value) -> Result<Uuid, KernelError> {
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

/// Log a chat message to the conversation log.
///
/// WHY separate helper: All chat messages (user and head) must be logged for
/// context persistence. Centralizing prevents divergence in log format.
async fn log_chat(
    scope: &str,
    kind: &str,
    content: &str,
    reply_to: Uuid,
    actor: &str,
) {
    let Some(k) = Kernel::get() else {
        return;
    };
    let dispatcher = k.dispatcher().await;
    let req = Frame::req(
        "log:append",
        json!({
            "kind": kind,
            "scope": scope,
            "data": {
                "content": content,
                "reply_to": reply_to.to_string(),
                "sender": actor,
            }
        }),
    )
    .with_actor(actor.to_string());
    let mut rx = dispatcher.dispatch(
        req,
        k.workspace().to_path_buf(),
        tokio_util::sync::CancellationToken::new(),
    );
    let _ = rx.recv().await;
}

// =============================================================================
// CHAT:MESSAGE - User and head messages
// =============================================================================
//
// WHY dual semantics: User messages initiate work (log + enqueue need), while
// head messages stream output (log + emit to turn). Keeping them unified reduces
// API surface, though it requires runtime actor validation.
//
// SECURITY NOTE: Actor validation prevents unauthorized need creation. Only
// "user" and "head/*" actors are allowed; other actors return error.

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
        ctx.check_cancelled()?;
        let scope = parse_scope(&data)?;
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

        // -------------------------------------------------------------------------
        // HEAD MESSAGES: Log and emit to turn stream
        // WHY this branch: Head-authored messages represent LLM output intended
        // for the user. They must be logged and forwarded to the turn stream.
        // -------------------------------------------------------------------------
        if actor.starts_with("head/") {
            log_chat(scope, "chat:head", &content, reply_to, actor).await;

            let Some(k) = Kernel::get() else {
                return Err(KernelError::internal("kernel not initialized"));
            };
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
        // -------------------------------------------------------------------------
        // USER MESSAGES: Log and enqueue need
        // WHY this branch: User-authored messages initiate work. They must be
        // logged and trigger need creation for head processing.
        // -------------------------------------------------------------------------
        } else if actor == "user" || actor.starts_with("human/") {
            log_chat(scope, "chat:user", &content, reply_to, actor).await;

            let Some(k) = Kernel::get() else {
                return Err(KernelError::internal("kernel not initialized"));
            };
            let dispatcher = k.dispatcher().await;
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
            let mut rx = dispatcher.dispatch(
                req,
                k.workspace().to_path_buf(),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx.recv().await;
        } else {
            return Err(KernelError::invalid_args(
                "chat:message actor must be user or head/*",
            ));
        }

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"sent": true}))).await;
        Ok(())
    }
}

// =============================================================================
// CHAT:TOOL - External tool call emission
// =============================================================================
//
// WHY this exists: The refactor removed `FrameOp::Redirect` in favor of explicit
// external tool call syscalls. This enables batching multiple tools before
// `chat:done`, which was impossible with the old redirect-closes-connection model.
//
// TRADE-OFF: Tool calls require registration in turn runtime for rendezvous.
// This adds complexity but ensures strict correlation and prevents stale results.

pub struct ChatTool;

#[async_trait]
impl Syscall for ChatTool {
    fn name(&self) -> &'static str {
        "chat:tool"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let scope = parse_scope(&data)?;
        let reply_to = parse_reply_to(&data)?;
        let tool_call_id = data
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if tool_call_id.is_empty() {
            return Err(KernelError::invalid_args("tool_call_id is required"));
        }
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            return Err(KernelError::invalid_args("name is required"));
        }
        let arguments = data.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
        if !arguments.is_object() {
            return Err(KernelError::invalid_args("arguments must be an object"));
        }

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let key = TurnKey::new(scope, reply_to);
        k.turns()
            .register_external_tool(&key, tool_call_id, name)
            .await
            .map_err(KernelError::invalid_args)?;

        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::item(
                    ctx.call_id,
                    json!({
                        "type": "tool_call",
                        "tool_call_id": tool_call_id,
                        "name": name,
                        "arguments": arguments,
                    }),
                )
                .with_name("chat:tool")
                .with_actor(ctx.actor_str().to_string()),
            )
            .await;

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"sent": true}))).await;
        Ok(())
    }
}

// =============================================================================
// CHAT:TOOL_RESULT - External tool result delivery
// =============================================================================
//
// WHY this exists: Resumes a paused turn after the client executes external
// tools. Delivers results to the waiting head via rendezvous mechanism without
// creating a new need.
//
// SECURITY NOTE: Validates tool_call_id against registered pending calls to
// prevent injection of fake results. Unknown tool_call_id returns error.

pub struct ChatToolResult;

#[async_trait]
impl Syscall for ChatToolResult {
    fn name(&self) -> &'static str {
        "chat:tool_result"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let scope = parse_scope(&data)?;
        let reply_to = parse_reply_to(&data)?;
        let tool_call_id = data
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if tool_call_id.is_empty() {
            return Err(KernelError::invalid_args("tool_call_id is required"));
        }
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            return Err(KernelError::invalid_args("name is required"));
        }
        let content = data
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::invalid_args("content is required"))?
            .to_string();
        let is_error = data
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let key = TurnKey::new(scope, reply_to);
        let cancelled = k.turns().is_cancelled(&key).await;

        k.turns()
            .deliver_external_tool_result(&key, tool_call_id, name, content.clone(), is_error)
            .await
            .map_err(KernelError::invalid_args)?;

        let dispatcher = k.dispatcher().await;
        let req = Frame::req(
            "log:append",
            json!({
                "kind": "chat:tool_result",
                "scope": scope,
                "data": {
                    "tool_call_id": tool_call_id,
                    "name": name,
                    "content": content,
                    "is_error": is_error,
                }
            }),
        )
        .with_actor(ctx.actor_str().to_string());
        let mut rx = dispatcher.dispatch(
            req,
            k.workspace().to_path_buf(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;

        if cancelled {
            return Err(KernelError::cancelled("turn cancelled"));
        }

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"delivered": true})))
            .await;
        Ok(())
    }
}

// =============================================================================
// CHAT:DONE - Turn segment completion
// =============================================================================
//
// WHY this exists: Terminates the current segment (client connection) but does
// not necessarily end the turn. If `reason == "awaiting_tools"`, the need
// remains paused and will resume when tool results arrive.
//
// TRADE-OFF: Reason field is strictly validated ("complete" or "awaiting_tools").
// This prevents ambiguous states but requires careful coordination between head
// and syscall layer.

pub struct ChatDone;

#[async_trait]
impl Syscall for ChatDone {
    fn name(&self) -> &'static str {
        "chat:done"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let scope = parse_scope(&data)?;
        let reply_to = parse_reply_to(&data)?;
        let reason = data
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if reason != "complete" && reason != "awaiting_tools" {
            return Err(KernelError::invalid_args("invalid reason"));
        }

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::item(
                    ctx.call_id,
                    json!({"type": "done", "reason": reason}),
                )
                .with_name("chat:done")
                .with_actor(ctx.actor_str().to_string()),
            )
            .await;
        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::done(ctx.call_id)
                    .with_name("chat:done")
                    .with_actor(ctx.actor_str().to_string()),
            )
            .await;
        k.sigcalls().close(scope, reply_to).await;

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"closed": true})))
            .await;
        Ok(())
    }
}

// =============================================================================
// CHAT:ERROR - Turn segment failure
// =============================================================================
//
// WHY this exists: Emits a terminal error frame to the turn stream and closes
// the segment. Unlike `chat:done`, signals that processing failed and should not
// resume.

pub struct ChatError;

#[async_trait]
impl Syscall for ChatError {
    fn name(&self) -> &'static str {
        "chat:error"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let scope = parse_scope(&data)?;
        let reply_to = parse_reply_to(&data)?;
        let code = data
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if code.is_empty() {
            return Err(KernelError::invalid_args("code is required"));
        }
        let message = data
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if message.is_empty() {
            return Err(KernelError::invalid_args("message is required"));
        }

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        k.sigcalls()
            .send(
                scope,
                reply_to,
                Frame::error(
                    ctx.call_id,
                    json!({"code": code, "message": message}),
                )
                .with_name("chat:error")
                .with_actor(ctx.actor_str().to_string()),
            )
            .await;
        k.sigcalls().close(scope, reply_to).await;

        let _ = tx.send(Frame::ok(ctx.call_id, json!({"closed": true}))).await;
        Ok(())
    }
}

// =============================================================================
// CHAT:CANCEL - Turn cancellation request
// =============================================================================
//
// WHY this exists: Allows clients to signal disconnection and request
// best-effort cancellation of in-flight work. Heads check cancellation at
// checkpoint boundaries (before LLM calls, before tool dispatch).
//
// TRADE-OFF: Cancellation is best-effort, not guaranteed. In-flight LLM requests
// or internal tool tasks may still complete. This balance avoids complex unwinding
// logic while still preventing wasteful work in most cases.

pub struct ChatCancel;

#[async_trait]
impl Syscall for ChatCancel {
    fn name(&self) -> &'static str {
        "chat:cancel"
    }

    async fn execute(
        &self,
        ctx: &SyscallContext,
        data: serde_json::Value,
        tx: mpsc::Sender<Frame>,
    ) -> Result<(), KernelError> {
        ctx.check_cancelled()?;
        let scope = parse_scope(&data)?;
        let reply_to = parse_reply_to(&data)?;
        let reason = data
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("client_disconnect")
            .trim();

        let Some(k) = Kernel::get() else {
            return Err(KernelError::internal("kernel not initialized"));
        };
        let key = TurnKey::new(scope, reply_to);
        k.turns().cancel(&key, reason).await;

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"cancelled": true})))
            .await;
        Ok(())
    }
}

// =============================================================================
// REGISTRATION
// =============================================================================

pub fn register(dispatcher: &mut KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(ChatMessage));
    dispatcher.register(Arc::new(ChatTool));
    dispatcher.register(Arc::new(ChatToolResult));
    dispatcher.register(Arc::new(ChatDone));
    dispatcher.register(Arc::new(ChatError));
    dispatcher.register(Arc::new(ChatCancel));
}
