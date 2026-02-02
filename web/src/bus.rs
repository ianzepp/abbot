// WebSocket bus integration for real-time updates.
//
// Connects to the backend WebSocket endpoint and processes incoming messages,
// routing them to the appropriate state updates. This replaces the useBus hook
// from the React frontend.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

use crate::state::{
    AppState, HandInfo, HeadInfo, Message, Need, StatusBarData, Task, ToolActivity, Want,
};

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsMessage {
    #[serde(rename = "connected")]
    Connected { data: ConnectedData },
    #[serde(rename = "bus")]
    Bus { data: Message },
    #[serde(rename = "pong")]
    Pong { data: PongData },
}

#[derive(Deserialize)]
struct ConnectedData {
    version: String,
}

#[derive(Deserialize)]
struct PongData {
    #[allow(dead_code)]
    timestamp: u64,
}

fn get_ws_url() -> String {
    let window = web_sys::window().expect("no window");
    let location = window.location();
    let protocol = location.protocol().unwrap_or_else(|_| "http:".to_string());
    let host = location
        .host()
        .unwrap_or_else(|_| "localhost:8080".to_string());

    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    format!("{}//{}/ws", ws_protocol, host)
}

pub fn use_bus(state: AppState) {
    let ws: Rc<RefCell<Option<WebSocket>>> = Rc::new(RefCell::new(None));
    let reconnect_timeout: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));

    let ws_clone = ws.clone();
    let reconnect_clone = reconnect_timeout.clone();

    Effect::new(move |_| {
        connect(state.clone(), ws_clone.clone(), reconnect_clone.clone());
    });
}

fn connect(
    state: AppState,
    ws_cell: Rc<RefCell<Option<WebSocket>>>,
    reconnect_timeout: Rc<RefCell<Option<i32>>>,
) {
    let url = get_ws_url();

    let ws = match WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(e) => {
            web_sys::console::error_1(&format!("Failed to create WebSocket: {:?}", e).into());
            schedule_reconnect(state, ws_cell, reconnect_timeout);
            return;
        }
    };

    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

    let state_clone = state.clone();
    let onopen = Closure::<dyn Fn()>::new(move || {
        web_sys::console::log_1(&"Bus connected".into());
        state_clone.connected.set(true);
    });
    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    onopen.forget();

    let state_clone = state.clone();
    let ws_cell_clone = ws_cell.clone();
    let reconnect_timeout_clone = reconnect_timeout.clone();
    let onclose = Closure::<dyn Fn(CloseEvent)>::new(move |_: CloseEvent| {
        web_sys::console::log_1(&"Bus disconnected".into());
        state_clone.connected.set(false);
        schedule_reconnect(
            state_clone.clone(),
            ws_cell_clone.clone(),
            reconnect_timeout_clone.clone(),
        );
    });
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    onclose.forget();

    let onerror = Closure::<dyn Fn(ErrorEvent)>::new(move |e: ErrorEvent| {
        web_sys::console::error_1(&format!("Bus error: {:?}", e.message()).into());
    });
    ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    onerror.forget();

    let state_clone = state.clone();
    let onmessage = Closure::<dyn Fn(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(text) = e.data().as_string() {
            state_clone.add_raw_bus_message(text.clone());

            match serde_json::from_str::<WsMessage>(&text) {
                Ok(ws_msg) => process_ws_message(ws_msg, &state_clone),
                Err(err) => {
                    web_sys::console::error_1(
                        &format!("Failed to parse bus message: {}", err).into(),
                    );
                }
            }
        }
    });
    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    *ws_cell.borrow_mut() = Some(ws);
}

fn schedule_reconnect(
    state: AppState,
    ws_cell: Rc<RefCell<Option<WebSocket>>>,
    reconnect_timeout: Rc<RefCell<Option<i32>>>,
) {
    let window = web_sys::window().expect("no window");

    let reconnect_timeout_for_callback = reconnect_timeout.clone();
    let callback = Closure::<dyn Fn()>::new(move || {
        connect(state.clone(), ws_cell.clone(), reconnect_timeout_for_callback.clone());
    });

    let timeout_id = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            2000,
        )
        .unwrap_or(0);

    callback.forget();
    *reconnect_timeout.borrow_mut() = Some(timeout_id);
}

fn process_ws_message(ws_msg: WsMessage, state: &AppState) {
    match ws_msg {
        WsMessage::Connected { data } => {
            web_sys::console::log_1(&format!("Connected to server v{}", data.version).into());
        }
        WsMessage::Bus { data } => {
            process_bus_message(data, state);
        }
        WsMessage::Pong { .. } => {}
    }
}

fn process_bus_message(msg: Message, state: &AppState) {
    match msg.op.as_str() {
        "Chat" => {
            if msg.scope == "main" {
                state.add_message(msg);
            }
        }
        "Need" => process_need_message(&msg, state),
        "Want" => process_want_message(&msg, state),
        "Task" => process_task_message(&msg, state),
        "Status" => {
            if let Some(status) = msg.data.get("Status") {
                if let Ok(stats) = serde_json::from_value::<StatusBarData>(status.clone()) {
                    state.status_bar.set(stats);
                }
            }
        }
        "Sleep" | "Wake" | "Idle" => process_head_lifecycle(&msg, state),
        "Done" => {
            state.tool_activity.set(None);
        }
        _ => {}
    }
}

fn process_need_message(msg: &Message, state: &AppState) {
    let need_data = msg.data.get("Need").unwrap_or(&msg.data);

    if let Some(request) = need_data.get("Request") {
        if let (Some(need_id), Some(need), Some(source), Some(priority), Some(context)) = (
            request.get("need_id").and_then(|v| v.as_str()),
            request.get("need").and_then(|v| v.as_str()),
            request.get("source").and_then(|v| v.as_str()),
            request.get("priority").and_then(|v| v.as_str()),
            request.get("context").and_then(|v| v.as_str()),
        ) {
            state.update_need(
                need_id,
                Need {
                    id: need_id.to_string(),
                    source: source.to_string(),
                    priority: priority.to_string(),
                    need: need.to_string(),
                    context: context.to_string(),
                    created_at: msg.timestamp,
                },
            );
        }
    } else if let Some(fulfilled) = need_data.get("Fulfilled") {
        if let Some(need_id) = fulfilled.get("need_id").and_then(|v| v.as_str()) {
            state.remove_need(need_id);
            if msg.scope == "main" {
                state.add_message(msg.clone());
            }
        }
    } else if let Some(expired) = need_data.get("Expired") {
        if let Some(need_id) = expired.get("need_id").and_then(|v| v.as_str()) {
            state.remove_need(need_id);
        }
    }
}

fn process_want_message(msg: &Message, state: &AppState) {
    let want_data = msg.data.get("Want").unwrap_or(&msg.data);

    if let Some(added) = want_data.get("Added") {
        if let (Some(want_id), Some(want), Some(context), Some(priority), Some(source)) = (
            added.get("want_id").and_then(|v| v.as_str()),
            added.get("want").and_then(|v| v.as_str()),
            added.get("context").and_then(|v| v.as_str()),
            added.get("priority").and_then(|v| v.as_str()),
            added.get("source").and_then(|v| v.as_str()),
        ) {
            state.update_want(
                want_id,
                Want {
                    id: want_id.to_string(),
                    want: want.to_string(),
                    context: context.to_string(),
                    priority: priority.to_string(),
                    source: source.to_string(),
                    created_at: msg.timestamp,
                },
            );
        }
    } else if let Some(removed) = want_data.get("Removed") {
        if let Some(want_id) = removed.get("want_id").and_then(|v| v.as_str()) {
            state.remove_want(want_id);
        }
    }
}

fn process_task_message(msg: &Message, state: &AppState) {
    let task_data = msg.data.get("Task").unwrap_or(&msg.data);

    if let Some(request) = task_data.get("Request") {
        if let (Some(task_id), Some(head_id), Some(goal)) = (
            request.get("task_id").and_then(|v| v.as_str()),
            request.get("head_id").and_then(|v| v.as_str()),
            request.get("goal").and_then(|v| v.as_str()),
        ) {
            let notify_scope = request
                .get("notify_scope")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            state.update_task(
                task_id,
                Task {
                    id: task_id.to_string(),
                    head_id: head_id.to_string(),
                    goal: goal.to_string(),
                    notify_scope,
                },
            );
        }
    } else if let Some(assigned) = task_data.get("Assigned") {
        if let (Some(hand_id), Some(task_id), Some(head_id)) = (
            assigned.get("hand_id").and_then(|v| v.as_str()),
            assigned.get("task_id").and_then(|v| v.as_str()),
            assigned.get("head_id").and_then(|v| v.as_str()),
        ) {
            state.update_hand(
                hand_id,
                HandInfo {
                    hand_id: hand_id.to_string(),
                    state: serde_json::json!({
                        "state": "running",
                        "task_id": task_id,
                        "head_id": head_id
                    }),
                },
            );
        }
    } else if let Some(tool_call) = task_data.get("ToolCall") {
        if let Some(tool) = tool_call.get("tool").and_then(|v| v.as_str()) {
            let args = tool_call
                .get("args")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let text = format_tool_activity_call(tool, &args);
            state.tool_activity.set(Some(ToolActivity {
                text,
                ts: msg.timestamp,
            }));
        }
    } else if let Some(tool_done) = task_data.get("ToolDone") {
        if let (Some(tool), Some(ok), Some(duration_ms)) = (
            tool_done.get("tool").and_then(|v| v.as_str()),
            tool_done.get("ok").and_then(|v| v.as_bool()),
            tool_done.get("duration_ms").and_then(|v| v.as_u64()),
        ) {
            let error_code = tool_done.get("error_code").and_then(|v| v.as_str());
            let text = format_tool_activity_done(tool, ok, duration_ms, error_code);
            state.tool_activity.set(Some(ToolActivity {
                text,
                ts: msg.timestamp,
            }));
        }
    } else if let Some(result) = task_data.get("Result") {
        if let Some(task_id) = result.get("task_id").and_then(|v| v.as_str()) {
            state.remove_task(task_id);

            if let Some(hand_id) = result.get("hand_id").and_then(|v| v.as_str()) {
                if hand_id.starts_with("hand-") {
                    state.update_hand(
                        hand_id,
                        HandInfo {
                            hand_id: hand_id.to_string(),
                            state: serde_json::json!({ "state": "idle" }),
                        },
                    );
                }
            }

            if msg.scope == "main" {
                state.add_message(msg.clone());
            }
        }
    }
}

fn process_head_lifecycle(msg: &Message, state: &AppState) {
    let head_id = &msg.sender;

    match msg.op.as_str() {
        "Sleep" | "Wake" => {
            state.update_head(
                head_id,
                HeadInfo {
                    head_id: head_id.clone(),
                    state: serde_json::json!({ "state": "available" }),
                },
            );
        }
        _ => {}
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

fn format_tool_args(args: &serde_json::Value) -> String {
    let obj = match args.as_object() {
        Some(o) => o,
        None => return String::new(),
    };

    let preferred = [
        ("path", "path"),
        ("pattern", "pattern"),
        ("query", "query"),
        ("url", "url"),
        ("command_preview", "cmd"),
        ("offset", "offset"),
        ("limit", "limit"),
    ];

    let mut parts = Vec::new();
    for (key, label) in preferred {
        if let Some(v) = obj.get(key) {
            if let Some(s) = v.as_str() {
                parts.push(format!("{}={}", label, clip(s, 64)));
            } else if let Some(n) = v.as_i64() {
                parts.push(format!("{}={}", label, n));
            } else if let Some(b) = v.as_bool() {
                parts.push(format!("{}={}", label, b));
            }
            if parts.len() >= 2 {
                break;
            }
        }
    }

    if !parts.is_empty() {
        return parts.join(" ");
    }

    if let Some(keys) = obj.get("keys").and_then(|v| v.as_array()) {
        let key_strs: Vec<&str> = keys.iter().filter_map(|k| k.as_str()).take(4).collect();
        if !key_strs.is_empty() {
            return format!("keys={}", key_strs.join(","));
        }
    }

    String::new()
}

fn format_tool_activity_call(tool: &str, args: &serde_json::Value) -> String {
    let detail = format_tool_args(args);
    if detail.is_empty() {
        tool.to_string()
    } else {
        format!("{} {}", tool, detail)
    }
}

fn format_tool_activity_done(
    tool: &str,
    ok: bool,
    duration_ms: u64,
    error_code: Option<&str>,
) -> String {
    let dur = format!("{}ms", duration_ms);
    if ok {
        format!("{} done ({})", tool, dur)
    } else {
        let code = error_code.map(|c| format!(" {}", c)).unwrap_or_default();
        format!("{} failed{} ({})", tool, code, dur)
    }
}
