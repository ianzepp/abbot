//! Tool-to-Syscall Dispatch Bridge
//!
//! Maps LLM tool calls (`tool__fs_write`) to kernel syscalls (`fs:write`),
//! dispatches them through the kernel dispatcher, and collects response frames
//! into a JSON string result suitable for returning to the LLM.
//!
//! ## Convention
//!
//! Tool names follow the pattern `tool__<ns>_<verb>`. The dispatch bridge strips
//! the `tool__` prefix and replaces the first `_` with `:` to derive the syscall
//! name. For example:
//!
//! - `tool__fs_write` → `fs:write`
//! - `tool__llm_chat` → `llm:chat`
//! - `tool__session_model_set` → `session:model_set`
//!
//! A small overrides table handles cases where the tool name doesn't match the
//! syscall name (e.g., `tool__task_create` → `task:enqueue`).
//!
//! ## Catalogs
//!
//! Each agent type gets a catalog of tool specs loaded from co-located JSON files.
//! The convention is `src/syscalls/<ns>/<verb>.json` next to `<verb>.rs`.

use std::path::Path;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::kernel::{Frame, FrameOp};
use crate::hal::llm::ToolSpec;
use crate::runtime::Kernel;

// ---------------------------------------------------------------------------
// Tool-to-syscall name mapping
// ---------------------------------------------------------------------------

/// Overrides for tool names that don't follow the `tool__<ns>_<verb>` → `<ns>:<verb>` convention.
static OVERRIDES: &[(&str, &str)] = &[
    ("tool__task_create", "task:enqueue"),
    ("tool__need_create", "need:enqueue"),
];

/// Convert a tool name to a syscall name.
///
/// Checks the overrides table first, then applies the convention:
/// `"tool__fs_write"` → strip `"tool__"` → `"fs_write"` → first `_` becomes `:` → `"fs:write"`
///
/// Returns `None` if the name doesn't start with `tool__` or has no `_` separator after the prefix.
pub fn tool_to_syscall(name: &str) -> Option<String> {
    // Check overrides first
    for (tool, syscall) in OVERRIDES {
        if name == *tool {
            return Some(syscall.to_string());
        }
    }

    let rest = name.strip_prefix("tool__")?;
    let idx = rest.find('_')?;
    let (ns, verb) = rest.split_at(idx);
    let verb = &verb[1..]; // skip the '_'
    if ns.is_empty() || verb.is_empty() {
        return None;
    }
    Some(format!("{ns}:{verb}"))
}

// ---------------------------------------------------------------------------
// Tool effect classification
// ---------------------------------------------------------------------------

/// Whether a tool call is read-only or mutating.
/// Used by callers to decide whether to acquire session locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffect {
    ReadOnly,
    Mutating,
}

/// Classify a tool's effect. Returns `None` for unknown tools.
pub fn tool_effect(name: &str) -> Option<ToolEffect> {
    match name {
        // Mutating tools
        "tool__fs_write"
        | "tool__fs_mkdir"
        | "tool__patch_apply"
        | "tool__config_update"
        | "tool__stm_update"
        | "tool__ltm_update"
        | "tool__task_create"
        | "tool__need_create"
        | "tool__want_create"
        | "tool__want_remove"
        | "tool__want_promote"
        | "tool__room_request" => Some(ToolEffect::Mutating),

        // Potentially mutating (depend on args, treat as mutating for locking)
        "tool__exec_run" | "tool__git_run" | "tool__net_fetch" => Some(ToolEffect::Mutating),

        // Read-only tools
        "tool__fs_read"
        | "tool__fs_list"
        | "tool__fs_search"
        | "tool__fs_diff"
        | "tool__text_echo"
        | "tool__llm_chat"
        | "tool__state_query"
        | "tool__stm_read"
        | "tool__config_read"
        | "tool__docs_list"
        | "tool__docs_search"
        | "tool__docs_read"
        | "tool__models_list"
        | "tool__tool_explain"
        | "tool__task_list"
        | "tool__task_read"
        | "tool__task_search"
        | "tool__want_list"
        | "tool__noop_signal"
        | "tool__noop_done" => Some(ToolEffect::ReadOnly),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Catalogs — tool specs for each agent type
// ---------------------------------------------------------------------------

/// Head agent tools: full access to all operations.
pub fn head_catalog() -> Vec<ToolSpec> {
    vec![
        // fs
        tool_spec!("fs/read"),
        tool_spec!("fs/write"),
        tool_spec!("fs/list"),
        tool_spec!("fs/mkdir"),
        // git / net / patch
        tool_spec!("git/run"),
        tool_spec!("net/fetch"),
        tool_spec!("patch/apply"),
        // llm
        tool_spec!("llm/chat"),
        // state / stm / config
        tool_spec!("state/query"),
        tool_spec!("stm/read"),
        tool_spec!("stm/update"),
        tool_spec!("config/read"),
        tool_spec!("config/update"),
        // docs
        tool_spec!("docs/list"),
        tool_spec!("docs/search"),
        tool_spec!("docs/read"),
        // models / tool
        tool_spec!("models/list"),
        tool_spec!("tool/explain"),
        // tasks
        tool_spec!("task/create"),
        tool_spec!("task/list"),
        tool_spec!("task/read"),
        tool_spec!("task/search"),
        // exec
        tool_spec!("exec/run"),
        // room
        tool_spec!("room/request"),
    ]
}

/// Hand agent tools: primarily read-only operations + execution tools.
pub fn hand_catalog() -> Vec<ToolSpec> {
    vec![
        tool_spec!("fs/read"),
        tool_spec!("fs/write"),
        tool_spec!("fs/list"),
        tool_spec!("fs/search"),
        tool_spec!("fs/diff"),
        tool_spec!("fs/mkdir"),
        tool_spec!("text/echo"),
        tool_spec!("net/fetch"),
        tool_spec!("git/run"),
        tool_spec!("patch/apply"),
        tool_spec!("llm/chat"),
    ]
}

/// Mind agent tools: strategic operations (wants, needs, LTM).
pub fn mind_catalog() -> Vec<ToolSpec> {
    vec![
        tool_spec!("ltm/update"),
        tool_spec!("need/create"),
        tool_spec!("want/list"),
        tool_spec!("want/create"),
        tool_spec!("want/remove"),
        tool_spec!("want/promote"),
        tool_spec!("llm/chat"),
    ]
}

/// Room agent tools: base room tools (noop/signal, noop/done) + strategic mind tools.
pub fn room_catalog() -> Vec<ToolSpec> {
    vec![
        // Room coordination
        tool_spec!("noop/signal"),
        tool_spec!("noop/done"),
        // Strategic operations (from mind_catalog)
        tool_spec!("ltm/update"),
        tool_spec!("need/create"),
        tool_spec!("want/list"),
        tool_spec!("want/create"),
        tool_spec!("want/remove"),
        tool_spec!("want/promote"),
        tool_spec!("llm/chat"),
    ]
}

/// Mind loop tools: proactive observer palette (superset of mind_catalog + introspection + noop).
pub fn mind_loop_catalog() -> Vec<ToolSpec> {
    vec![
        // Strategic operations (from mind_catalog)
        tool_spec!("ltm/update"),
        tool_spec!("need/create"),
        tool_spec!("want/list"),
        tool_spec!("want/create"),
        tool_spec!("want/remove"),
        tool_spec!("want/promote"),
        tool_spec!("llm/chat"),
        // Read-only introspection
        tool_spec!("task/list"),
        tool_spec!("state/query"),
        // Room dispatch
        tool_spec!("room/request"),
        // Termination
        tool_spec!("noop/signal"),
    ]
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Generate a human-readable markdown description of tool specs.
pub fn describe_tools(specs: &[ToolSpec]) -> String {
    let mut out = String::new();
    for spec in specs {
        let f = &spec.function;
        out.push_str(&format!("### {}\n", f.name));
        if let Some(desc) = &f.description {
            out.push_str(&format!("{desc}\n\n"));
        }
        if let Some(props) = f.parameters.get("properties").and_then(|p| p.as_object()) {
            if !props.is_empty() {
                out.push_str("Parameters:\n");
                for (name, schema) in props {
                    let desc = schema
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    let ty = schema
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("any");
                    out.push_str(&format!("- `{name}` ({ty}): {desc}\n"));
                }
                out.push('\n');
            }
        }
    }
    out
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
            tool_to_syscall("tool__session_model_set"),
            Some("session:model_set".into())
        );
    }

    #[test]
    fn test_tool_to_syscall_overrides() {
        assert_eq!(
            tool_to_syscall("tool__task_create"),
            Some("task:enqueue".into())
        );
        assert_eq!(
            tool_to_syscall("tool__need_create"),
            Some("need:enqueue".into())
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

    #[test]
    fn test_tool_effect() {
        assert_eq!(tool_effect("tool__fs_write"), Some(ToolEffect::Mutating));
        assert_eq!(tool_effect("tool__fs_read"), Some(ToolEffect::ReadOnly));
        assert_eq!(tool_effect("tool__git_run"), Some(ToolEffect::Mutating));
        assert_eq!(tool_effect("tool__llm_chat"), Some(ToolEffect::ReadOnly));
        assert_eq!(tool_effect("unknown_tool"), None);
    }

    #[test]
    fn test_head_catalog_loads() {
        let specs = head_catalog();
        assert!(!specs.is_empty());
        assert!(specs.iter().any(|s| s.function.name == "tool__fs_read"));
        assert!(specs.iter().any(|s| s.function.name == "tool__task_create"));
        assert!(specs.iter().any(|s| s.function.name == "tool__room_request"));
    }

    #[test]
    fn test_hand_catalog_loads() {
        let specs = hand_catalog();
        assert!(!specs.is_empty());
        assert!(specs.iter().any(|s| s.function.name == "tool__fs_read"));
        assert!(specs.iter().any(|s| s.function.name == "tool__fs_search"));
        assert!(specs.iter().any(|s| s.function.name == "tool__llm_chat"));
    }

    #[test]
    fn test_mind_catalog_loads() {
        let specs = mind_catalog();
        assert!(!specs.is_empty());
        assert!(specs.iter().any(|s| s.function.name == "tool__ltm_update"));
        assert!(specs.iter().any(|s| s.function.name == "tool__want_create"));
        assert!(specs.iter().any(|s| s.function.name == "tool__need_create"));
    }

    #[test]
    fn test_room_catalog_loads() {
        let specs = room_catalog();
        assert!(!specs.is_empty());
        // Room coordination
        assert!(specs.iter().any(|s| s.function.name == "tool__noop_signal"));
        assert!(specs.iter().any(|s| s.function.name == "tool__noop_done"));
        // Strategic ops
        assert!(specs.iter().any(|s| s.function.name == "tool__ltm_update"));
        assert!(specs.iter().any(|s| s.function.name == "tool__need_create"));
    }

    #[test]
    fn test_mind_loop_catalog_loads() {
        let specs = mind_loop_catalog();
        assert!(!specs.is_empty());
        // Strategic ops from mind_catalog
        assert!(specs.iter().any(|s| s.function.name == "tool__ltm_update"));
        assert!(specs.iter().any(|s| s.function.name == "tool__need_create"));
        assert!(specs.iter().any(|s| s.function.name == "tool__want_create"));
        // Introspection
        assert!(specs.iter().any(|s| s.function.name == "tool__task_list"));
        assert!(specs.iter().any(|s| s.function.name == "tool__state_query"));
        // Room dispatch
        assert!(specs.iter().any(|s| s.function.name == "tool__room_request"));
        // Termination
        assert!(specs.iter().any(|s| s.function.name == "tool__noop_signal"));
    }
}
