// WebSocket connection for real-time kernel frame streaming.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

use crate::state::AppState;

#[derive(Clone, Debug, Deserialize)]
pub struct Frame {
    pub id: String,
    pub op: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsMessage {
    #[serde(rename = "connected")]
    Connected { data: ConnectedData },
    #[serde(rename = "frame")]
    Frame { data: Frame },
    #[serde(rename = "pong")]
    Pong { data: PongData },
    #[serde(rename = "error")]
    Error { data: ErrorData },
}

#[derive(Deserialize)]
struct ConnectedData {
    version: String,
}

#[derive(Deserialize)]
struct PongData {
    #[allow(dead_code)]
    timestamp_ms: i64,
}

#[derive(Deserialize)]
struct ErrorData {
    message: String,
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
        web_sys::console::log_1(&"WebSocket connected".into());
        state_clone.connected.set(true);
    });
    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    onopen.forget();

    let state_clone = state.clone();
    let ws_cell_clone = ws_cell.clone();
    let reconnect_timeout_clone = reconnect_timeout.clone();
    let onclose = Closure::<dyn Fn(CloseEvent)>::new(move |_: CloseEvent| {
        web_sys::console::log_1(&"WebSocket disconnected".into());
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
        web_sys::console::error_1(&format!("WebSocket error: {:?}", e.message()).into());
    });
    ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    onerror.forget();

    let state_clone = state.clone();
    let onmessage = Closure::<dyn Fn(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(text) = e.data().as_string() {
            match serde_json::from_str::<WsMessage>(&text) {
                Ok(ws_msg) => process_ws_message(ws_msg, &state_clone),
                Err(err) => {
                    web_sys::console::error_1(
                        &format!("Failed to parse message: {}", err).into(),
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
        WsMessage::Frame { data } => {
            state.add_frame(data);
        }
        WsMessage::Pong { .. } => {}
        WsMessage::Error { data } => {
            web_sys::console::error_1(&format!("Server error: {}", data.message).into());
        }
    }
}
