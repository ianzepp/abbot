use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::kernel::{Frame, KernelDispatcher, KernelError, Syscall, SyscallContext, TurnKey};
use crate::runtime::Kernel;

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
            .send(scope, reply_to, Frame::done(ctx.call_id))
            .await;
        k.sigcalls().close(scope, reply_to).await;

        let _ = tx
            .send(Frame::ok(ctx.call_id, json!({"closed": true})))
            .await;
        Ok(())
    }
}

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

pub fn register(dispatcher: &mut KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(ChatMessage));
    dispatcher.register(Arc::new(ChatTool));
    dispatcher.register(Arc::new(ChatToolResult));
    dispatcher.register(Arc::new(ChatDone));
    dispatcher.register(Arc::new(ChatError));
    dispatcher.register(Arc::new(ChatCancel));
}
