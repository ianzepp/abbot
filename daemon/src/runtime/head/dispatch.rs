use super::HeadService;
use super::types::ActiveNeed;
use crate::runtime::Kernel;
use serde_json::json;
use uuid::Uuid;

impl HeadService {
    /// Fulfill a need by dispatching need:fulfill syscall.
    pub(super) async fn fulfill_need(&self, need: &ActiveNeed, summary: &str) {
        if let Some(k) = Kernel::get() {
            let dispatcher = k.dispatcher().await;
            let req = crate::kernel::Frame::req(
                "need:fulfill",
                serde_json::json!({"need_id": need.need_id.clone(), "summary": summary}),
            )
            .with_actor(format!("head/{}", self.head_id));
            let mut rx = dispatcher.dispatch(
                req,
                self.workspace_root.clone(),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx.recv().await;
        }

        tracing::debug!(
            head = %self.head_id,
            need_id = %need.need_id,
            scope = %need.scope.as_deref().unwrap_or("main"),
            reply_to = ?need.reply_to,
            "need fulfilled"
        );
    }

    pub(super) async fn send_error(&self, need: &ActiveNeed, message: &str) {
        tracing::error!(
            head = %self.head_id,
            need_id = %need.need_id,
            scope = %need.scope.as_deref().unwrap_or("main"),
            "{}",
            message
        );
        if let (Some(k), Some(reply_to)) = (Kernel::get(), need.reply_to) {
            let scope = need.scope.as_deref().unwrap_or("main");
            let dispatcher = k.dispatcher().await;
            let req = crate::kernel::Frame::req(
                "chat:error",
                json!({
                    "scope": scope,
                    "reply_to": reply_to.to_string(),
                    "code": "E_HEAD",
                    "message": message,
                }),
            )
            .with_actor(format!("head/{}", self.head_id));
            let mut rx = dispatcher.dispatch(
                req,
                self.workspace_root.clone(),
                tokio_util::sync::CancellationToken::new(),
            );
            let _ = rx.recv().await;
        }
    }

    /// Emit a chat message via chat:message syscall.
    pub(super) async fn emit_chat_message(&self, scope: &str, reply_to: Uuid, content: &str) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = crate::kernel::Frame::req(
            "chat:message",
            json!({
                "scope": scope,
                "reply_to": reply_to.to_string(),
                "content": content,
            }),
        )
        .with_actor(format!("head/{}", self.head_id));
        let mut rx = dispatcher.dispatch(
            req,
            self.workspace_root.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;
        Ok(())
    }

    /// Emit an external tool call via chat:tool syscall.
    pub(super) async fn emit_chat_tool(
        &self,
        scope: &str,
        reply_to: Uuid,
        tool_call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = crate::kernel::Frame::req(
            "chat:tool",
            json!({
                "scope": scope,
                "reply_to": reply_to.to_string(),
                "tool_call_id": tool_call_id,
                "name": name,
                "arguments": arguments,
            }),
        )
        .with_actor(format!("head/{}", self.head_id));
        let mut rx = dispatcher.dispatch(
            req,
            self.workspace_root.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;
        Ok(())
    }

    /// Emit chat:done to close the current segment.
    pub(super) async fn emit_chat_done(&self, scope: &str, reply_to: Uuid, reason: &str) -> Result<(), String> {
        let Some(k) = Kernel::get() else {
            return Err("kernel not initialized".to_string());
        };
        let dispatcher = k.dispatcher().await;
        let req = crate::kernel::Frame::req(
            "chat:done",
            json!({
                "scope": scope,
                "reply_to": reply_to.to_string(),
                "reason": reason,
            }),
        )
        .with_actor(format!("head/{}", self.head_id));
        let mut rx = dispatcher.dispatch(
            req,
            self.workspace_root.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        let _ = rx.recv().await;
        Ok(())
    }

    /// Check if the turn has been cancelled.
    pub(super) async fn is_turn_cancelled(&self, need: &ActiveNeed) -> bool {
        let (Some(k), Some(reply_to)) = (Kernel::get(), need.reply_to) else {
            return false;
        };
        let scope = need.scope.as_deref().unwrap_or("main");
        let key = crate::kernel::TurnKey::new(scope, reply_to);
        k.turns().is_cancelled(&key).await
    }

}
