// Center panel with tabs for different views.

use leptos::prelude::*;
use std::collections::HashMap;
use wasm_bindgen::JsCast;

use crate::bus::send_message;
use crate::components::TabBar;
use crate::state::{AppState, TabType};

#[component]
pub fn CenterPanel() -> impl IntoView {
    let state = expect_context::<AppState>();

    let active_tab = move || {
        let active_id = state.active_tab.get();
        state.tabs.get().into_iter().find(|t| t.id == active_id)
    };

    view! {
        <div class="center-panel">
            <TabBar />
            <div class="center-content">
                {move || {
                    match active_tab().map(|t| t.tab_type) {
                        Some(TabType::Chat) => view! { <ChatView /> }.into_any(),
                        Some(TabType::Activity) => view! { <ActivityView /> }.into_any(),
                        None => view! { <div class="empty-view">"No tab selected"</div> }.into_any(),
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn ChatView() -> impl IntoView {
    let state = expect_context::<AppState>();
    let input_value = RwSignal::new(String::new());

    #[derive(Clone)]
    struct ChatItem {
        key: String,
        role: &'static str,
        scope: String,
        text: String,
    }

    let chat_items = move || {
        let frames = state.frames.get();

        let mut out: Vec<ChatItem> = Vec::new();
        let mut buffers: HashMap<String, String> = HashMap::new();
        let mut thread_scope: HashMap<String, String> = HashMap::new();
        let mut thread_seq: HashMap<String, usize> = HashMap::new();

        let flush = |thread_id: &str,
                     buffers: &mut HashMap<String, String>,
                     thread_scope: &mut HashMap<String, String>,
                     thread_seq: &mut HashMap<String, usize>,
                     out: &mut Vec<ChatItem>| {
            let Some(text) = buffers.remove(thread_id) else {
                return;
            };
            if text.trim().is_empty() {
                return;
            }

            let seq = thread_seq.entry(thread_id.to_string()).or_insert(0);
            *seq += 1;
            let scope = thread_scope
                .get(thread_id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            out.push(ChatItem {
                key: format!("assistant:{}:{}", thread_id, seq),
                role: "assistant",
                scope,
                text,
            });
        };

        // Oldest -> newest
        for f in frames.into_iter().rev() {
            // User message is represented by need:enqueue.
            if f.op == "req" && f.name.as_deref() == Some("need:enqueue") {
                let scope = f
                    .data
                    .as_ref()
                    .and_then(|d| d.get("scope"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let text = f
                    .data
                    .as_ref()
                    .and_then(|d| d.get("need"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !text.trim().is_empty() {
                    out.push(ChatItem {
                        key: format!("user:{}", f.id),
                        role: "user",
                        scope,
                        text,
                    });
                }
                continue;
            }

            // Assistant streamed output.
            if f.op == "bytes" && f.name.as_deref() == Some("chat:message") {
                let thread_id = f.parent_id.clone().unwrap_or_else(|| f.id.clone());
                let text = f
                    .data
                    .as_ref()
                    .and_then(|d| d.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !text.is_empty() {
                    buffers
                        .entry(thread_id.clone())
                        .or_insert_with(String::new)
                        .push_str(text);
                }
                let scope = f
                    .actor
                    .clone()
                    .or_else(|| {
                        f.data
                            .as_ref()
                            .and_then(|d| d.get("scope"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| "unknown".to_string());
                thread_scope.insert(thread_id, scope);
                continue;
            }

            // Tool request terminates the assistant stream; flush whatever we've got.
            if f.op == "redirect" && f.name.as_deref() == Some("tool:request") {
                if let Some(thread_id) = f.parent_id.clone() {
                    flush(
                        &thread_id,
                        &mut buffers,
                        &mut thread_scope,
                        &mut thread_seq,
                        &mut out,
                    );
                    let scope = f.actor.clone().unwrap_or_else(|| "unknown".to_string());
                    let tool_name = f
                        .data
                        .as_ref()
                        .and_then(|d| d.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool");
                    out.push(ChatItem {
                        key: format!("assistant:{}:tool", thread_id),
                        role: "assistant",
                        scope,
                        text: format!("[tool:request] {}", tool_name),
                    });
                }
                continue;
            }

            // Done means the current assistant message (if any) is complete.
            if f.op == "done" {
                if let Some(thread_id) = f.parent_id.clone() {
                    flush(
                        &thread_id,
                        &mut buffers,
                        &mut thread_scope,
                        &mut thread_seq,
                        &mut out,
                    );
                }
                continue;
            }
        }

        // Flush any trailing buffers (e.g., abrupt disconnect).
        let trailing = buffers.keys().cloned().collect::<Vec<_>>();
        for thread_id in trailing {
            flush(
                &thread_id,
                &mut buffers,
                &mut thread_scope,
                &mut thread_seq,
                &mut out,
            );
        }

        out
    };

    let on_submit = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        let text = input_value.get();
        if !text.trim().is_empty() {
            send_message(&text, None);
            input_value.set(String::new());
        }
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            let text = input_value.get();
            if !text.trim().is_empty() {
                send_message(&text, None);
                input_value.set(String::new());
            }
        }
    };

    view! {
        <div class="chat-view">
            <div class="chat-messages">
                <For
                    each=chat_items
                    key=|item| item.key.clone()
                    children=move |item| {
                        let cls = format!("chat-message chat-message-{}", item.role);
                        view! {
                            <div class=cls>
                                {(item.scope != "main" && item.scope != "unknown").then(|| view! {
                                    <div class="chat-message-meta">{item.scope.clone()}</div>
                                })}
                                <div class="chat-message-text">{item.text}</div>
                            </div>
                        }
                    }
                />
            </div>
            <form class="chat-input-form" on:submit=on_submit>
                <input
                    type="text"
                    class="chat-input"
                    placeholder="Send a message..."
                    prop:value=move || input_value.get()
                    on:input=move |ev| {
                        let target = ev.target().unwrap();
                        let input = target.unchecked_ref::<web_sys::HtmlInputElement>();
                        input_value.set(input.value());
                    }
                    on:keydown=on_keydown
                />
            </form>
        </div>
    }
}

#[component]
fn ActivityView() -> impl IntoView {
    let state = expect_context::<AppState>();

    let activity_frames = move || {
        state
            .frames
            .get()
            .into_iter()
            .filter(|f| {
                // Show high-signal frames
                matches!(f.op.as_str(), "req" | "ok" | "error" | "done" | "redirect")
            })
            .take(100)
            .collect::<Vec<_>>()
    };

    view! {
        <div class="activity-view">
            <For
                each=activity_frames
                key=|frame| frame.id.clone()
                children=move |frame| {
                    let name = frame.name.clone().unwrap_or_else(|| "-".to_string());
                    let actor = frame.actor.clone().unwrap_or_default();

                    view! {
                        <div class="activity-item">
                            <span class={format!("activity-op activity-op-{}", frame.op)}>{frame.op.clone()}</span>
                            <span class="activity-name">{name}</span>
                            {(!actor.is_empty()).then(|| view! {
                                <span class="activity-actor">{actor}</span>
                            })}
                        </div>
                    }
                }
            />
        </div>
    }
}
