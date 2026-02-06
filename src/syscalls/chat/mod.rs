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

use serde_json::json;
use uuid::Uuid;

use crate::kernel::{Frame, KernelError};
use crate::runtime::Kernel;

pub(crate) fn parse_scope(data: &serde_json::Value) -> Result<&str, KernelError> {
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

pub(crate) async fn log_chat(
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

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(ChatMessage));
    dispatcher.register(Arc::new(ChatTool));
    dispatcher.register(Arc::new(ChatToolResult));
    dispatcher.register(Arc::new(ChatDone));
    dispatcher.register(Arc::new(ChatError));
    dispatcher.register(Arc::new(ChatCancel));
}
