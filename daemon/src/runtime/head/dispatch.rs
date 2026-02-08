use super::HeadService;
use super::types::ActiveNeed;
use crate::runtime::Kernel;
use serde_json::json;

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
}
