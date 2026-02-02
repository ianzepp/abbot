// ChatPanel component - displays chat messages and input.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use super::ToolBar;
use crate::api::{get_messages, send_message};
use crate::state::{AppState, Message};

fn format_time(timestamp: u64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(timestamp as f64));
    let hours = date.get_hours();
    let minutes = date.get_minutes();
    format!("{:02}:{:02}", hours, minutes)
}

fn extract_text(data: &serde_json::Value) -> String {
    if let Some(text) = data.get("Text") {
        if let Some(s) = text.as_str() {
            return s.to_string();
        }
    }
    if let Some(s) = data.as_str() {
        return s.to_string();
    }
    if let Some(obj) = data.as_object() {
        if let Some(content) = obj.get("content") {
            if let Some(s) = content.as_str() {
                return s.to_string();
            }
        }
    }
    serde_json::to_string(data).unwrap_or_default()
}

#[component]
pub fn ChatPanel() -> impl IntoView {
    let state = expect_context::<AppState>();

    let (input, set_input) = signal(String::new());
    let (sending, set_sending) = signal(false);

    let messages = state.messages;
    let connected = state.connected;
    let set_messages = state.messages;

    Effect::new(move |_| {
        spawn_local(async move {
            match get_messages("main", 100).await {
                Ok(msgs) => set_messages.set(msgs),
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to load messages: {}", e).into())
                }
            }
        });
    });

    let handle_send = move || {
        let content = input.get();
        let trimmed = content.trim().to_string();
        if trimmed.is_empty() || sending.get() {
            return;
        }

        set_sending.set(true);
        spawn_local(async move {
            match send_message(&trimmed, "main").await {
                Ok(()) => set_input.set(String::new()),
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to send message: {}", e).into())
                }
            }
            set_sending.set(false);
        });
    };

    let chat_messages = move || {
        messages
            .get()
            .into_iter()
            .filter(|m| m.op == "Chat")
            .collect::<Vec<_>>()
    };

    view! {
        <div class="chat-panel-inner">
            <Show when=move || !connected.get()>
                <div class="chat-disconnected-banner">
                    "Disconnected from server"
                </div>
            </Show>

            <div class="chat-messages">
                <Show
                    when=move || !chat_messages().is_empty()
                    fallback=|| view! { <div class="empty-state">"No messages yet"</div> }
                >
                    <For
                        each=chat_messages
                        key=|msg| msg.id.clone()
                        let:msg
                    >
                        <ChatMessage message=msg />
                    </For>
                </Show>
            </div>

            <ToolBar />

            <div class="chat-input-container">
                <textarea
                    class="chat-input"
                    prop:value=move || input.get()
                    on:input=move |ev| {
                        let target = event_target::<web_sys::HtmlTextAreaElement>(&ev);
                        set_input.set(target.value());
                    }
                    on:keydown=move |ev| {
                        let key = ev.key();
                        if key == "Enter" && !ev.shift_key() {
                            ev.prevent_default();
                            handle_send();
                        }
                    }
                    placeholder="Type a message... (Enter to send, Shift+Enter for newline)"
                    rows="3"
                    disabled=move || sending.get()
                />
            </div>
        </div>
    }
}

#[component]
fn ChatMessage(message: Message) -> impl IntoView {
    let content = extract_text(&message.data);
    if content.is_empty() {
        return view! {}.into_any();
    }

    let sender_class = match message.origin.as_str() {
        "human" => "human",
        "head" => "head",
        "system" => "system",
        _ => "",
    };

    view! {
        <div class="chat-message">
            <div class="chat-message-header">
                <span class=format!("chat-message-sender {}", sender_class)>
                    {message.sender.clone()}
                </span>
                <span class="chat-message-time">
                    {format_time(message.timestamp)}
                </span>
            </div>
            <div class="chat-message-content">
                {content}
            </div>
        </div>
    }.into_any()
}
