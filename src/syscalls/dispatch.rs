//! Tool-to-Syscall Dispatch Bridge
//!
//! Maps LLM tool calls (`tool__fs_write`) to kernel syscalls (`fs:write`),
//! dispatches them through the kernel dispatcher, and collects response frames
//! into a JSON string result suitable for returning to the LLM.

use std::path::Path;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::kernel::{Frame, FrameOp};
use crate::runtime::Kernel;

/// Convert a tool name to a syscall name.
///
/// `"tool__fs_write"` → strip `"tool__"` → `"fs_write"` → first `_` becomes `:` → `"fs:write"`
///
/// Returns `None` if the name doesn't start with `tool__` or has no `_` separator after the prefix.
pub fn tool_to_syscall(name: &str) -> Option<String> {
    let rest = name.strip_prefix("tool__")?;
    let idx = rest.find('_')?;
    let (ns, verb) = rest.split_at(idx);
    let verb = &verb[1..]; // skip the '_'
    if ns.is_empty() || verb.is_empty() {
        return None;
    }
    Some(format!("{ns}:{verb}"))
}

/// Dispatch an LLM tool call through the kernel syscall system.
///
/// Maps the tool name to a syscall, dispatches a `Frame::req`, and collects
/// the response into a JSON string (`{"ok":true,"data":...}` or `{"ok":false,"error":...}`).
pub async fn dispatch_tool(
    name: &str,
    args_json: &str,
    actor: &str,
    cwd: &Path,
) -> String {
    let syscall_name = match tool_to_syscall(name) {
        Some(n) => n,
        None => {
            return json!({"ok": false, "error": {
                "code": "E_INVALID_ARGS",
                "message": format!("cannot map tool name to syscall: {name}")
            }})
            .to_string();
        }
    };

    let data: Value = match serde_json::from_str(args_json) {
        Ok(v) => v,
        Err(e) => {
            return json!({"ok": false, "error": {
                "code": "E_INVALID_ARGS",
                "message": format!("invalid JSON args: {e}")
            }})
            .to_string();
        }
    };

    let Some(k) = Kernel::get() else {
        return json!({"ok": false, "error": {
            "code": "E_INTERNAL",
            "message": "kernel not initialized"
        }})
        .to_string();
    };

    let dispatcher = k.dispatcher().await;
    let req = Frame::req(&syscall_name, data).with_actor(actor.to_string());
    let cancel = CancellationToken::new();
    let mut rx = dispatcher.dispatch(req, cwd.to_path_buf(), cancel);

    collect_response(&mut rx).await
}

/// Collect syscall response frames into a JSON string.
///
/// Handles Ok, Error, Item, and Done frames. Items are accumulated into an array.
/// The final result is `{"ok":true,"data":{...}}` or `{"ok":false,"error":{...}}`.
pub async fn collect_response(rx: &mut crate::kernel::KernelReceiver) -> String {
    let mut items: Vec<Value> = Vec::new();
    let mut result_data: Option<Value> = None;
    let mut error_data: Option<Value> = None;

    while let Some(frame) = rx.recv().await {
        match frame.op {
            FrameOp::Item => {
                if let Some(d) = frame.data {
                    items.push(d);
                }
            }
            FrameOp::Ok => {
                result_data = Some(frame.data.unwrap_or(json!({})));
                break;
            }
            FrameOp::Error => {
                error_data = Some(frame.data.unwrap_or(json!({"message": "syscall failed"})));
                break;
            }
            FrameOp::Done => break,
            _ => {}
        }
    }

    if let Some(err) = error_data {
        let code = err
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("E_SYSCALL");
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("syscall failed");
        return json!({"ok": false, "error": {"code": code, "message": message}}).to_string();
    }

    let mut data = result_data.unwrap_or(json!({}));
    if !items.is_empty() {
        if let Some(obj) = data.as_object_mut() {
            obj.insert("items".to_string(), json!(items));
        }
    }

    json!({"ok": true, "data": data}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_to_syscall_basic() {
        assert_eq!(tool_to_syscall("tool__fs_write"), Some("fs:write".into()));
        assert_eq!(tool_to_syscall("tool__fs_read"), Some("fs:read".into()));
        assert_eq!(
            tool_to_syscall("tool__git_run"),
            Some("git:run".into())
        );
        assert_eq!(
            tool_to_syscall("tool__net_fetch"),
            Some("net:fetch".into())
        );
        assert_eq!(
            tool_to_syscall("tool__task_enqueue"),
            Some("task:enqueue".into())
        );
    }

    #[test]
    fn test_tool_to_syscall_invalid() {
        assert_eq!(tool_to_syscall("head__fs_write"), None);
        assert_eq!(tool_to_syscall("hand__fs_read"), None);
        assert_eq!(tool_to_syscall("tool__"), None);
        assert_eq!(tool_to_syscall("tool__fs"), None);
        assert_eq!(tool_to_syscall("random"), None);
    }
}
